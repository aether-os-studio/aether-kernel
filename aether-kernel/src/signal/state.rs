extern crate alloc;

use alloc::sync::Arc;

use aether_frame::libs::spin::SpinLock;
use aether_vfs::{PollEvents, SharedWaitListener, WaitQueue};

use super::abi::{
    SIG_BLOCK, SIG_DFL, SIG_SETMASK, SIG_UNBLOCK, SIGCONT, SIGKILL, SIGNAL_MAX, SIGSTOP, SIGTSTP,
    SIGTTIN, SIGTTOU, SigSet, SignalAction, SignalInfo, sigbit,
};
use super::core::{SignalDelivery, delivery_for, sanitize_mask};

#[derive(Debug, Clone)]
struct SignalStateInner {
    blocked: SigSet,
    pending: [Option<SignalInfo>; SIGNAL_MAX + 1],
    actions: [SignalAction; SIGNAL_MAX + 1],
    suspend_mask: Option<SigSet>,
}

#[derive(Clone)]
pub struct SignalState {
    inner: Arc<SpinLock<SignalStateInner>>,
    waiters: Arc<WaitQueue>,
}

impl SignalState {
    pub fn new() -> Self {
        let mut actions = [SignalAction {
            handler: SIG_DFL,
            flags: 0,
            restorer: 0,
            mask: 0,
        }; SIGNAL_MAX + 1];
        let mut signal = 1usize;
        while signal <= SIGNAL_MAX {
            actions[signal] = SignalAction::default_for(signal as u8);
            signal += 1;
        }

        Self::from_inner(SignalStateInner {
            blocked: 0,
            pending: [None; SIGNAL_MAX + 1],
            actions,
            suspend_mask: None,
        })
    }

    fn from_inner(inner: SignalStateInner) -> Self {
        Self {
            inner: Arc::new(SpinLock::new(inner)),
            waiters: Arc::new(WaitQueue::new()),
        }
    }

    pub fn blocked(&self) -> SigSet {
        self.inner.lock().blocked
    }

    pub fn set_blocked_mask(&mut self, mask: SigSet) {
        self.inner.lock().blocked = sanitize_mask(mask);
    }

    pub fn restore_mask(&mut self, mask: SigSet) {
        self.inner.lock().blocked = sanitize_mask(mask);
    }

    pub fn fork_copy(&self) -> Self {
        let inner = self.inner.lock();
        Self::from_inner(SignalStateInner {
            blocked: inner.blocked,
            pending: [None; SIGNAL_MAX + 1],
            actions: inner.actions,
            suspend_mask: None,
        })
    }

    pub fn prepare_for_exec(&mut self) {
        let mut inner = self.inner.lock();
        let mut signal = 1usize;
        while signal <= SIGNAL_MAX {
            let action = inner.actions[signal];
            if action.handler != super::abi::SIG_DFL && action.handler != super::abi::SIG_IGN {
                inner.actions[signal] = SignalAction::default_for(signal as u8);
            }
            signal += 1;
        }
        inner.suspend_mask = None;
    }

    pub fn set_mask(&mut self, how: u64, set: SigSet) {
        let mut inner = self.inner.lock();
        match how {
            SIG_BLOCK => inner.blocked |= sanitize_mask(set),
            SIG_UNBLOCK => inner.blocked &= !sanitize_mask(set),
            SIG_SETMASK => inner.blocked = sanitize_mask(set),
            _ => {}
        }
    }

    pub fn action(&self, signal: u8) -> Option<SignalAction> {
        self.inner.lock().actions.get(signal as usize).copied()
    }

    pub fn set_action(&mut self, signal: u8, action: SignalAction) -> bool {
        let index = signal as usize;
        if index == 0 || index > SIGNAL_MAX || signal == SIGKILL || signal == SIGSTOP {
            return false;
        }
        self.inner.lock().actions[index] = action;
        true
    }

    pub fn enqueue(&mut self, info: SignalInfo) {
        let index = info.signal as usize;
        if index == 0 || index > SIGNAL_MAX {
            return;
        }

        let mut inner = self.inner.lock();
        if info.signal == SIGCONT {
            inner.pending[SIGSTOP as usize] = None;
            inner.pending[SIGTSTP as usize] = None;
            inner.pending[SIGTTIN as usize] = None;
            inner.pending[SIGTTOU as usize] = None;
        } else if matches!(info.signal, SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU) {
            inner.pending[SIGCONT as usize] = None;
        }

        inner.pending[index] = Some(info);
        drop(inner);
        self.waiters.notify(PollEvents::READ);
    }

    #[allow(dead_code)]
    pub fn has_unblocked_pending(&self) -> bool {
        let inner = self.inner.lock();
        inner
            .pending
            .iter()
            .flatten()
            .any(|info| !super::core::is_blocked(inner.blocked, info.signal))
    }

    pub fn has_deliverable(&self, handlers_supported: bool) -> bool {
        let inner = self.inner.lock();
        let mut signal = 1usize;
        while signal <= SIGNAL_MAX {
            if let Some(info) = inner.pending[signal] {
                let action = inner.actions[signal];
                if !matches!(
                    delivery_for(inner.blocked, action, info, handlers_supported),
                    SignalDelivery::None
                ) {
                    return true;
                }
            }
            signal += 1;
        }
        false
    }

    pub fn take_next_delivery(&mut self, handlers_supported: bool) -> SignalDelivery {
        let mut inner = self.inner.lock();
        let mut signal = 1usize;
        while signal <= SIGNAL_MAX {
            if let Some(info) = inner.pending[signal] {
                let action = inner.actions[signal];
                match delivery_for(inner.blocked, action, info, handlers_supported) {
                    SignalDelivery::None => {}
                    delivery => {
                        inner.pending[signal] = None;
                        return delivery;
                    }
                }
            }
            signal += 1;
        }
        SignalDelivery::None
    }

    pub fn enter_sigsuspend(&mut self, mask: SigSet) {
        let mut inner = self.inner.lock();
        inner.suspend_mask = Some(inner.blocked);
        inner.blocked = sanitize_mask(mask);
    }

    pub fn leave_sigsuspend(&mut self) {
        let mut inner = self.inner.lock();
        if let Some(previous) = inner.suspend_mask.take() {
            inner.blocked = previous;
        }
    }

    pub fn has_pending_in_mask(&self, mask: SigSet) -> bool {
        let inner = self.inner.lock();
        let masked = sanitize_mask(mask);
        inner.pending.iter().flatten().any(|info| {
            let bit = sigbit(info.signal);
            bit != 0 && (masked & bit) != 0
        })
    }

    pub fn take_pending_in_mask(&self, mask: SigSet) -> Option<SignalInfo> {
        let masked = sanitize_mask(mask);
        let mut inner = self.inner.lock();
        let mut signal = 1usize;
        while signal <= SIGNAL_MAX {
            if let Some(info) = inner.pending[signal] {
                let bit = sigbit(info.signal);
                if bit != 0 && (masked & bit) != 0 {
                    inner.pending[signal] = None;
                    return Some(info);
                }
            }
            signal += 1;
        }
        None
    }

    pub fn register_waiter(&self, listener: SharedWaitListener) -> u64 {
        self.waiters.register(PollEvents::READ, listener)
    }

    pub fn unregister_waiter(&self, waiter_id: u64) -> bool {
        self.waiters.unregister(waiter_id)
    }

    pub fn notify_waiters(&self) {
        self.waiters.notify(PollEvents::READ);
    }

    #[allow(dead_code)]
    pub fn has_waitable_child_signal(&self) -> bool {
        let inner = self.inner.lock();
        matches!(
            inner.pending[super::abi::SIGCHLD as usize],
            Some(SignalInfo {
                signal: super::abi::SIGCHLD,
                ..
            })
        )
    }
}

impl Default for SignalState {
    fn default() -> Self {
        Self::new()
    }
}
