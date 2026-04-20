extern crate alloc;

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

use aether_frame::libs::spin::SpinLock;
use aether_vfs::{PollEvents, SharedWaitListener, WaitQueue};

use super::abi::{
    SA_ONSTACK, SIG_BLOCK, SIG_DFL, SIG_SETMASK, SIG_UNBLOCK, SIGCONT, SIGKILL, SIGNAL_MAX,
    SIGSTOP, SIGTSTP, SIGTTIN, SIGTTOU, SS_AUTODISARM, SS_ONSTACK, SigSet, SignalAction,
    SignalInfo, SignalStack, sigbit, signal_altstack_config_enabled, signal_altstack_contains_sp,
    signal_altstack_disable, signal_altstack_format_old, signal_altstack_store,
    signal_altstack_validate_new, signal_stack_base,
};
use super::core::{SignalDelivery, delivery_for, sanitize_mask};
use crate::errno::SysErr;

#[derive(Debug, Clone)]
struct SignalStateInner {
    blocked: SigSet,
    pending: [Option<SignalInfo>; SIGNAL_MAX + 1],
    suspend_mask: Option<SigSet>,
    altstack: SignalStack,
}

#[derive(Clone)]
pub struct SignalState {
    actions: Arc<SpinLock<[SignalAction; SIGNAL_MAX + 1]>>,
    inner: Arc<SpinLock<SignalStateInner>>,
    waiters: Arc<WaitQueue>,
    version: Arc<AtomicU64>,
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

        Self::from_parts(
            Arc::new(SpinLock::new(actions)),
            SignalStateInner {
                blocked: 0,
                pending: [None; SIGNAL_MAX + 1],
                suspend_mask: None,
                altstack: SignalStack::disabled(),
            },
        )
    }

    fn from_parts(
        actions: Arc<SpinLock<[SignalAction; SIGNAL_MAX + 1]>>,
        inner: SignalStateInner,
    ) -> Self {
        Self {
            actions,
            inner: Arc::new(SpinLock::new(inner)),
            waiters: Arc::new(WaitQueue::new()),
            version: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn clone_for_thread(&self) -> Self {
        let inner = self.inner.lock();
        Self::from_parts(
            self.actions.clone(),
            SignalStateInner {
                blocked: inner.blocked,
                pending: [None; SIGNAL_MAX + 1],
                suspend_mask: None,
                altstack: inner.altstack,
            },
        )
    }

    pub fn fork_copy(&self) -> Self {
        let inner = self.inner.lock();
        let actions = *self.actions.lock();
        Self::from_parts(
            Arc::new(SpinLock::new(actions)),
            SignalStateInner {
                blocked: inner.blocked,
                pending: [None; SIGNAL_MAX + 1],
                suspend_mask: None,
                altstack: inner.altstack,
            },
        )
    }

    fn bump(&self) {
        let _ = self.version.fetch_add(1, Ordering::AcqRel);
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
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

    pub fn prepare_for_exec(&mut self) {
        let mut actions = self.actions.lock();
        let mut signal = 1usize;
        while signal <= SIGNAL_MAX {
            let action = actions[signal];
            if action.handler != super::abi::SIG_DFL && action.handler != super::abi::SIG_IGN {
                actions[signal] = SignalAction::default_for(signal as u8);
            }
            signal += 1;
        }
        drop(actions);

        let mut inner = self.inner.lock();
        inner.suspend_mask = None;
        signal_altstack_disable(&mut inner.altstack);
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
        self.actions.lock().get(signal as usize).copied()
    }

    pub fn set_action(&mut self, signal: u8, action: SignalAction) -> bool {
        let index = signal as usize;
        if index == 0 || index > SIGNAL_MAX || signal == SIGKILL || signal == SIGSTOP {
            return false;
        }
        self.actions.lock()[index] = action;
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
        self.bump();
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
        let actions = self.actions.lock();
        let inner = self.inner.lock();
        let mut signal = 1usize;
        while signal <= SIGNAL_MAX {
            if let Some(info) = inner.pending[signal] {
                let action = actions[signal];
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
        let actions = self.actions.lock();
        let mut inner = self.inner.lock();
        let mut signal = 1usize;
        while signal <= SIGNAL_MAX {
            if let Some(info) = inner.pending[signal] {
                let action = actions[signal];
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
                    drop(inner);
                    self.bump();
                    return Some(info);
                }
            }
            signal += 1;
        }
        None
    }

    pub fn altstack(&self, user_sp: u64) -> SignalStack {
        let inner = self.inner.lock();
        signal_altstack_format_old(&inner.altstack, user_sp)
    }

    pub fn set_altstack(
        &mut self,
        new_stack: Option<SignalStack>,
        user_sp: u64,
    ) -> Result<SignalStack, SysErr> {
        let mut inner = self.inner.lock();
        let old_stack = signal_altstack_format_old(&inner.altstack, user_sp);

        if let Some(new_stack) = new_stack {
            signal_altstack_validate_new(&new_stack)?;
            if (old_stack.ss_flags & SS_ONSTACK) != 0 {
                return Err(SysErr::Perm);
            }
            signal_altstack_store(&mut inner.altstack, &new_stack);
        }

        Ok(old_stack)
    }

    pub fn restore_after_sigreturn(&mut self, mask: SigSet, altstack: SignalStack) {
        let mut inner = self.inner.lock();
        inner.blocked = sanitize_mask(mask);
        signal_altstack_store(&mut inner.altstack, &altstack);
    }

    pub fn should_use_altstack_for_signal(&self, action_flags: u64, user_sp: u64) -> bool {
        let inner = self.inner.lock();
        (action_flags & SA_ONSTACK) != 0
            && signal_altstack_config_enabled(&inner.altstack)
            && !signal_altstack_contains_sp(&inner.altstack, user_sp)
    }

    pub fn altstack_top_for_signal(&self) -> Option<u64> {
        let inner = self.inner.lock();
        signal_stack_base(&inner.altstack).checked_add(inner.altstack.ss_size)
    }

    pub fn arm_altstack_for_signal(&mut self) {
        let mut inner = self.inner.lock();
        if (inner.altstack.ss_flags & SS_AUTODISARM) != 0 {
            signal_altstack_disable(&mut inner.altstack);
        }
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
