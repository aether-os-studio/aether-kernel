use crate::errno::{SysErr, SysResult};

pub(crate) const CLONE_VM: u64 = 0x0000_0100;
pub(crate) const CLONE_FS: u64 = 0x0000_0200;
pub(crate) const CLONE_FILES: u64 = 0x0000_0400;
pub(crate) const CLONE_SIGHAND: u64 = 0x0000_0800;
pub(crate) const CLONE_PIDFD: u64 = 0x0000_1000;
pub(crate) const CLONE_PTRACE: u64 = 0x0000_2000;
pub(crate) const CLONE_VFORK: u64 = 0x0000_4000;
pub(crate) const CLONE_PARENT: u64 = 0x0000_8000;
pub(crate) const CLONE_THREAD: u64 = 0x0001_0000;
pub(crate) const CLONE_NEWNS: u64 = 0x0002_0000;
pub(crate) const CLONE_SYSVSEM: u64 = 0x0004_0000;
pub(crate) const CLONE_SETTLS: u64 = 0x0008_0000;
pub(crate) const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
pub(crate) const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
pub(crate) const CLONE_DETACHED: u64 = 0x0040_0000;
pub(crate) const CLONE_UNTRACED: u64 = 0x0080_0000;
pub(crate) const CLONE_CHILD_SETTID: u64 = 0x0100_0000;
pub(crate) const CLONE_NEWCGROUP: u64 = 0x0200_0000;
pub(crate) const CLONE_NEWUTS: u64 = 0x0400_0000;
pub(crate) const CLONE_NEWIPC: u64 = 0x0800_0000;
pub(crate) const CLONE_NEWUSER: u64 = 0x1000_0000;
pub(crate) const CLONE_NEWPID: u64 = 0x2000_0000;
pub(crate) const CLONE_NEWNET: u64 = 0x4000_0000;
pub(crate) const CLONE_IO: u64 = 0x8000_0000;
pub(crate) const CLONE_CLEAR_SIGHAND: u64 = 0x0001_0000_0000;
pub(crate) const CLONE_INTO_CGROUP: u64 = 0x0002_0000_0000;

const CSIGNAL_MASK: u64 = 0xff;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloneParams {
    pub flags: u64,
    pub exit_signal: u64,
    pub child_stack_pointer: Option<u64>,
    pub parent_tid: Option<u64>,
    pub child_tid: Option<u64>,
    pub tls: Option<u64>,
}

impl CloneParams {
    pub(crate) const fn fork() -> Self {
        Self {
            flags: 0,
            exit_signal: 17,
            child_stack_pointer: None,
            parent_tid: None,
            child_tid: None,
            tls: None,
        }
    }

    pub(crate) const fn vfork() -> Self {
        Self {
            flags: CLONE_VM | CLONE_VFORK,
            exit_signal: 17,
            child_stack_pointer: None,
            parent_tid: None,
            child_tid: None,
            tls: None,
        }
    }

    pub(crate) fn from_clone(
        flags_and_signal: u64,
        child_stack: u64,
        parent_tid: u64,
        child_tid: u64,
        tls: u64,
    ) -> Self {
        let flags = flags_and_signal & !CSIGNAL_MASK;
        let exit_signal = flags_and_signal & CSIGNAL_MASK;
        Self {
            flags,
            exit_signal,
            child_stack_pointer: (child_stack != 0).then_some(child_stack),
            parent_tid: ((flags & CLONE_PARENT_SETTID) != 0).then_some(parent_tid),
            child_tid: ((flags & (CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID)) != 0)
                .then_some(child_tid),
            tls: ((flags & CLONE_SETTLS) != 0).then_some(tls),
        }
    }

    pub(crate) fn from_clone3(args: LinuxCloneArgs) -> Self {
        let child_stack_pointer = if args.stack != 0 {
            let top = if args.stack_size != 0 {
                args.stack.saturating_add(args.stack_size)
            } else {
                args.stack
            };
            Some(top)
        } else {
            None
        };

        Self {
            flags: args.flags,
            exit_signal: args.exit_signal & CSIGNAL_MASK,
            child_stack_pointer,
            parent_tid: ((args.flags & CLONE_PARENT_SETTID) != 0).then_some(args.parent_tid),
            child_tid: ((args.flags & (CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID)) != 0)
                .then_some(args.child_tid),
            tls: ((args.flags & CLONE_SETTLS) != 0).then_some(args.tls),
        }
    }

    pub(crate) fn validate(self) -> SysResult<()> {
        let unsupported_flags = CLONE_PIDFD
            | CLONE_PTRACE
            | CLONE_NEWNS
            | CLONE_DETACHED
            | CLONE_UNTRACED
            | CLONE_NEWCGROUP
            | CLONE_NEWUTS
            | CLONE_NEWIPC
            | CLONE_NEWUSER
            | CLONE_NEWPID
            | CLONE_NEWNET
            | CLONE_IO
            | CLONE_CLEAR_SIGHAND
            | CLONE_INTO_CGROUP;

        if (self.flags & unsupported_flags) != 0 {
            // These require kernel subsystems that do not exist yet:
            // shared fs/fd tables, thread groups, namespaces, pidfd, cgroup placement, ptrace.
            return Err(SysErr::Inval);
        }
        if (self.flags & CLONE_THREAD) != 0
            && ((self.flags & CLONE_SIGHAND) == 0 || (self.flags & CLONE_VM) == 0)
        {
            return Err(SysErr::Inval);
        }
        if (self.flags & CLONE_SIGHAND) != 0 && (self.flags & CLONE_VM) == 0 {
            return Err(SysErr::Inval);
        }
        if (self.flags & CLONE_VFORK) != 0 && (self.flags & CLONE_VM) == 0 {
            return Err(SysErr::Inval);
        }
        if (self.flags & CLONE_THREAD) != 0 && self.exit_signal != 0 {
            return Err(SysErr::Inval);
        }
        if (self.flags & CLONE_PARENT_SETTID) != 0 && self.parent_tid.is_none() {
            return Err(SysErr::Fault);
        }
        if (self.flags & (CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID)) != 0
            && self.child_tid.is_none()
        {
            return Err(SysErr::Fault);
        }
        if (self.flags & CLONE_SETTLS) != 0 && self.tls.is_none() {
            return Err(SysErr::Fault);
        }
        if self.exit_signal > CSIGNAL_MASK {
            return Err(SysErr::Inval);
        }
        Ok(())
    }

    pub(crate) const fn shares_vm(self) -> bool {
        (self.flags & CLONE_VM) != 0
    }

    pub(crate) const fn is_vfork(self) -> bool {
        (self.flags & CLONE_VFORK) != 0
    }

    pub(crate) const fn inherit_parent(self) -> bool {
        (self.flags & CLONE_PARENT) != 0
    }

    pub(crate) const fn set_tls(self) -> bool {
        (self.flags & CLONE_SETTLS) != 0
    }

    pub(crate) const fn share_fs(self) -> bool {
        (self.flags & CLONE_FS) != 0
    }

    pub(crate) const fn share_files(self) -> bool {
        (self.flags & CLONE_FILES) != 0
    }

    pub(crate) const fn share_sighand(self) -> bool {
        (self.flags & CLONE_SIGHAND) != 0
    }

    pub(crate) const fn thread(self) -> bool {
        (self.flags & CLONE_THREAD) != 0
    }

    pub(crate) const fn share_sysvsem(self) -> bool {
        (self.flags & CLONE_SYSVSEM) != 0
    }

    pub(crate) const fn set_parent_tid(self) -> bool {
        (self.flags & CLONE_PARENT_SETTID) != 0
    }

    pub(crate) const fn set_child_tid(self) -> bool {
        (self.flags & CLONE_CHILD_SETTID) != 0
    }

    pub(crate) const fn clear_child_tid(self) -> bool {
        (self.flags & CLONE_CHILD_CLEARTID) != 0
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct LinuxCloneArgs {
    pub flags: u64,
    pub pidfd: u64,
    pub child_tid: u64,
    pub parent_tid: u64,
    pub exit_signal: u64,
    pub stack: u64,
    pub stack_size: u64,
    pub tls: u64,
    pub set_tid: u64,
    pub set_tid_size: u64,
    pub cgroup: u64,
}

impl LinuxCloneArgs {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();

    pub(crate) fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::SIZE {
            return None;
        }

        let mut words = [0u64; 11];
        for (index, chunk) in bytes[..Self::SIZE].chunks_exact(8).enumerate() {
            let mut raw = [0u8; 8];
            raw.copy_from_slice(chunk);
            words[index] = u64::from_ne_bytes(raw);
        }

        Some(Self {
            flags: words[0],
            pidfd: words[1],
            child_tid: words[2],
            parent_tid: words[3],
            exit_signal: words[4],
            stack: words[5],
            stack_size: words[6],
            tls: words[7],
            set_tid: words[8],
            set_tid_size: words[9],
            cgroup: words[10],
        })
    }

    pub(crate) fn validate(self, size: usize) -> SysResult<()> {
        if size < Self::SIZE {
            return Err(SysErr::Inval);
        }
        if self.pidfd != 0 || self.set_tid != 0 || self.set_tid_size != 0 || self.cgroup != 0 {
            // pidfd, set_tid and cgroup targeting need dedicated kernel infrastructure.
            return Err(SysErr::Inval);
        }
        if self.stack == 0 && self.stack_size != 0 {
            return Err(SysErr::Inval);
        }
        Ok(())
    }
}
