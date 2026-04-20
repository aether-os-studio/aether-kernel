#[path = "arch/arch_prctl.rs"]
mod arch_prctl;
#[path = "arch/iopl.rs"]
mod iopl;

#[path = "memory/brk.rs"]
mod brk;
#[path = "memory/mincore.rs"]
mod mincore;
#[path = "memory/mmap.rs"]
mod mmap;
#[path = "memory/mprotect.rs"]
mod mprotect;
#[path = "memory/mremap.rs"]
mod mremap;
#[path = "memory/munmap.rs"]
mod munmap;

#[path = "time/clock_getres.rs"]
mod clock_getres;
#[path = "time/clock_gettime.rs"]
mod clock_gettime;
#[path = "time/clock_nanosleep.rs"]
mod clock_nanosleep;
#[path = "time/gettimeofday.rs"]
mod gettimeofday;
#[path = "time/time.rs"]
mod time;
#[path = "time/timerfd.rs"]
mod timerfd;

#[path = "net/accept.rs"]
mod accept;
#[path = "net/bind.rs"]
mod bind;
#[path = "net/connect.rs"]
mod connect;
#[path = "net/getpeername.rs"]
mod getpeername;
#[path = "net/getsockname.rs"]
mod getsockname;
#[path = "net/getsockopt.rs"]
mod getsockopt;
#[path = "net/listen.rs"]
mod listen;
#[path = "net/recvfrom.rs"]
mod recvfrom;
#[path = "net/recvmsg.rs"]
mod recvmsg;
#[path = "net/sendmsg.rs"]
mod sendmsg;
#[path = "net/sendto.rs"]
mod sendto;
#[path = "net/setsockopt.rs"]
mod setsockopt;
#[path = "net/shutdown.rs"]
mod shutdown;
#[path = "net/socket.rs"]
mod socket;
#[path = "net/socket_common.rs"]
mod socket_common;
#[path = "net/socketpair.rs"]
mod socketpair;

#[path = "fs/access.rs"]
mod access;
#[path = "fs/chdir.rs"]
mod chdir;
#[path = "fs/chmod.rs"]
mod chmod;
#[path = "fs/chown.rs"]
mod chown;
#[path = "fs/chroot.rs"]
mod chroot;
#[path = "fs/creat.rs"]
mod creat;
#[path = "fs/faccessat.rs"]
mod faccessat;
#[path = "fs/faccessat2.rs"]
mod faccessat2;
#[path = "fs/fchdir.rs"]
mod fchdir;
#[path = "fs/fchmod.rs"]
mod fchmod;
#[path = "fs/fchmodat.rs"]
mod fchmodat;
#[path = "fs/fchown.rs"]
mod fchown;
#[path = "fs/fchownat.rs"]
mod fchownat;
#[path = "fs/fsopen.rs"]
mod fsopen;
#[path = "fs/getcwd.rs"]
mod getcwd;
#[path = "fs/inotify_add_watch.rs"]
mod inotify_add_watch;
#[path = "fs/inotify_init.rs"]
mod inotify_init;
#[path = "fs/inotify_init1.rs"]
mod inotify_init1;
#[path = "fs/inotify_rm_watch.rs"]
mod inotify_rm_watch;
#[path = "fs/lchown.rs"]
mod lchown;
#[path = "fs/link.rs"]
mod link;
#[path = "fs/linkat.rs"]
mod linkat;
#[path = "fs/lstat.rs"]
mod lstat;
#[path = "fs/mkdir.rs"]
mod mkdir;
#[path = "fs/mount.rs"]
mod mount;
#[path = "fs/newfstatat.rs"]
mod newfstatat;
#[path = "fs/open.rs"]
mod open;
#[path = "fs/openat.rs"]
mod openat;
#[path = "fs/pivot_root.rs"]
mod pivot_root;
#[path = "fs/readlink.rs"]
mod readlink;
#[path = "fs/readlinkat.rs"]
mod readlinkat;
#[path = "fs/rename.rs"]
mod rename;
#[path = "fs/renameat.rs"]
mod renameat;
#[path = "fs/stat.rs"]
mod stat;
#[path = "fs/statfs.rs"]
mod statfs;
#[path = "fs/statx.rs"]
mod statx;
#[path = "fs/symlink.rs"]
mod symlink;
#[path = "fs/umask.rs"]
mod umask;
#[path = "fs/umount2.rs"]
mod umount2;
#[path = "fs/unlink.rs"]
mod unlink;
#[path = "fs/unlinkat.rs"]
mod unlinkat;

#[path = "fd/close.rs"]
mod close;
#[path = "fd/close_range.rs"]
mod close_range;
#[path = "fd/dup.rs"]
mod dup;
#[path = "fd/dup2.rs"]
mod dup2;
#[path = "fd/dup3.rs"]
mod dup3;
#[path = "fd/epoll_create.rs"]
mod epoll_create;
#[path = "fd/epoll_ctl.rs"]
mod epoll_ctl;
#[path = "fd/epoll_pwait.rs"]
mod epoll_pwait;
#[path = "fd/epoll_wait.rs"]
mod epoll_wait;
#[path = "fd/eventfd.rs"]
mod eventfd;
#[path = "fd/fadvise64.rs"]
mod fadvise64;
#[path = "fd/fallocate.rs"]
mod fallocate;
#[path = "fd/fcntl.rs"]
mod fcntl;
#[path = "fd/flock.rs"]
mod flock;
#[path = "fd/fstat.rs"]
mod fstat;
#[path = "fd/fstatfs.rs"]
mod fstatfs;
#[path = "fd/getdents64.rs"]
mod getdents64;
#[path = "fd/ioctl.rs"]
mod ioctl;
#[path = "fd/lseek.rs"]
mod lseek;
#[path = "fd/memfd_create.rs"]
mod memfd_create;
#[path = "fd/pipe.rs"]
mod pipe;
#[path = "fd/pipe2.rs"]
mod pipe2;
#[path = "fd/poll.rs"]
mod poll;
#[path = "fd/ppoll.rs"]
mod ppoll;
#[path = "fd/pread64.rs"]
mod pread64;
#[path = "fd/preadv64.rs"]
mod preadv64;
#[path = "fd/pselect6.rs"]
mod pselect6;
#[path = "fd/pwrite64.rs"]
mod pwrite64;
#[path = "fd/pwritev64.rs"]
mod pwritev64;
#[path = "fd/read.rs"]
mod read;
#[path = "fd/readv.rs"]
mod readv;
#[path = "fd/sendfile.rs"]
mod sendfile;
#[path = "fd/write.rs"]
mod write;
#[path = "fd/writev.rs"]
mod writev;

#[path = "process/clone.rs"]
mod clone;
#[path = "process/clone3.rs"]
mod clone3;
#[path = "process/execve.rs"]
mod execve;
#[path = "process/execveat.rs"]
mod execveat;
#[path = "process/exit.rs"]
mod exit;
#[path = "process/exit_group.rs"]
mod exit_group;
#[path = "process/fork.rs"]
mod fork;
#[path = "process/futex.rs"]
mod futex;
#[path = "process/getegid.rs"]
mod getegid;
#[path = "process/geteuid.rs"]
mod geteuid;
#[path = "process/getgid.rs"]
mod getgid;
#[path = "process/getpgid.rs"]
mod getpgid;
#[path = "process/getpgrp.rs"]
mod getpgrp;
#[path = "process/getpid.rs"]
mod getpid;
#[path = "process/getppid.rs"]
mod getppid;
#[path = "process/getresgid.rs"]
mod getresgid;
#[path = "process/getresuid.rs"]
mod getresuid;
#[path = "process/gettid.rs"]
mod gettid;
#[path = "process/getuid.rs"]
mod getuid;
#[path = "process/prctl.rs"]
mod prctl;
#[path = "process/prlimit64.rs"]
mod prlimit64;
#[path = "process/sched_yield.rs"]
mod sched_yield;
#[path = "process/set_tid_address.rs"]
mod set_tid_address;
#[path = "process/setgid.rs"]
mod setgid;
#[path = "process/setgroups.rs"]
mod setgroups;
#[path = "process/setpgid.rs"]
mod setpgid;
#[path = "process/setresgid.rs"]
mod setresgid;
#[path = "process/setresuid.rs"]
mod setresuid;
#[path = "process/setsid.rs"]
mod setsid;
#[path = "process/setuid.rs"]
mod setuid;
#[path = "process/vfork.rs"]
mod vfork;
#[path = "process/wait4.rs"]
mod wait4;
#[path = "process/waitid.rs"]
mod waitid;

#[path = "signal/getrandom.rs"]
mod getrandom;
#[path = "signal/kill.rs"]
mod kill;
#[path = "signal/rseq.rs"]
mod rseq;
#[path = "signal/rt_sigaction.rs"]
mod rt_sigaction;
#[path = "signal/rt_sigprocmask.rs"]
mod rt_sigprocmask;
#[path = "signal/rt_sigreturn.rs"]
mod rt_sigreturn;
#[path = "signal/rt_sigsuspend.rs"]
mod rt_sigsuspend;
#[path = "signal/set_robust_list.rs"]
mod set_robust_list;
#[path = "signal/signalfd.rs"]
mod signalfd;
#[path = "signal/signalfd4.rs"]
mod signalfd4;
#[path = "signal/tgkill.rs"]
mod tgkill;
#[path = "signal/tkill.rs"]
mod tkill;
#[path = "signal/uname.rs"]
mod uname;

use alloc::string::String;
use alloc::vec::Vec;

use super::KernelSyscallContext;

pub fn init() {
    crate::register_syscalls!(
        super::registry::registry(),
        [
            write::WriteSyscall,
            access::AccessSyscall,
            getpid::GetPidSyscall,
            gettid::GetTidSyscall,
            getppid::GetPpidSyscall,
            getuid::GetUidSyscall,
            sched_yield::SchedYieldSyscall,
            exit::ExitSyscall,
            exit_group::ExitGroupSyscall,
            fork::ForkSyscall,
            clone::CloneSyscall,
            vfork::VforkSyscall,
            clone3::Clone3Syscall,
            wait4::Wait4Syscall,
            waitid::WaitidSyscall,
            kill::KillSyscall,
            lstat::LstatSyscall,
            lseek::LseekSyscall,
            readlink::ReadlinkSyscall,
            mount::MountSyscall,
            umount2::Umount2Syscall,
            pivot_root::PivotRootSyscall,
            execve::ExecveSyscall,
            execveat::ExecveAtSyscall,
            faccessat::FaccessAtSyscall,
            faccessat2::FaccessAt2Syscall,
            fadvise64::Fadvise64Syscall,
            pread64::Pread64Syscall,
            pwrite64::Pwrite64Syscall,
            read::ReadSyscall,
            socket::SocketSyscall,
            setsockopt::SetsockoptSyscall,
            getsockopt::GetsockoptSyscall,
            accept::AcceptSyscall,
            accept::Accept4Syscall,
            sendto::SendtoSyscall,
            sendmsg::SendmsgSyscall,
            recvfrom::RecvfromSyscall,
            recvmsg::RecvmsgSyscall,
            shutdown::ShutdownSyscall,
            bind::BindSyscall,
            listen::ListenSyscall,
            getsockname::GetsocknameSyscall,
            getpeername::GetpeernameSyscall,
            socketpair::SocketpairSyscall,
            readlinkat::ReadlinkAtSyscall,
            readv::ReadvSyscall,
            preadv64::Preadv64Syscall,
            pwritev64::Pwritev64Syscall,
            close::CloseSyscall,
            close_range::CloseRangeSyscall,
            creat::CreatSyscall,
            dup::DupSyscall,
            dup2::Dup2Syscall,
            dup3::Dup3Syscall,
            ioctl::IoctlSyscall,
            fcntl::FcntlSyscall,
            flock::FlockSyscall,
            fstat::FstatSyscall,
            fstatfs::FstatfsSyscall,
            futex::FutexSyscall,
            tkill::TkillSyscall,
            getcwd::GetcwdSyscall,
            getdents64::Getdents64Syscall,
            geteuid::GeteUidSyscall,
            getegid::GeteGidSyscall,
            getgid::GetGidSyscall,
            getpgrp::GetPgrpSyscall,
            getpgid::GetPgidSyscall,
            setsid::SetSidSyscall,
            getresgid::GetResGidSyscall,
            getresuid::GetResUidSyscall,
            getrandom::GetrandomSyscall,
            gettimeofday::GettimeofdaySyscall,
            clock_getres::ClockGetresSyscall,
            clock_gettime::ClockGettimeSyscall,
            clock_nanosleep::ClockNanosleepSyscall,
            time::TimeSyscall,
            timerfd::TimerfdCreateSyscall,
            timerfd::TimerfdSettimeSyscall,
            timerfd::TimerfdGettimeSyscall,
            iopl::IoplSyscall,
            brk::BrkSyscall,
            mincore::MincoreSyscall,
            chdir::ChdirSyscall,
            chown::ChownSyscall,
            chmod::ChmodSyscall,
            chroot::ChrootSyscall,
            connect::ConnectSyscall,
            fchdir::FchdirSyscall,
            fchown::FchownSyscall,
            fchownat::FchownAtSyscall,
            fchmod::FchmodSyscall,
            fchmodat::FchmodatSyscall,
            lchown::LchownSyscall,
            mkdir::MkdirSyscall,
            mmap::MmapSyscall,
            mprotect::MprotectSyscall,
            mremap::MremapSyscall,
            munmap::MunmapSyscall,
            open::OpenSyscall,
            openat::OpenAtSyscall,
            arch_prctl::ArchPrctlSyscall,
            rt_sigaction::RtSigactionSyscall,
            rt_sigprocmask::RtSigprocmaskSyscall,
            rt_sigreturn::RtSigreturnSyscall,
            rt_sigsuspend::RtSigsuspendSyscall,
            set_tid_address::SetTidAddressSyscall,
            set_robust_list::SetRobustListSyscall,
            rseq::RseqSyscall,
            pipe::PipeSyscall,
            eventfd::EventfdSyscall,
            pipe2::Pipe2Syscall,
            memfd_create::MemfdCreateSyscall,
            signalfd::SignalfdSyscall,
            signalfd4::Signalfd4Syscall,
            eventfd::Eventfd2Syscall,
            fallocate::FallocateSyscall,
            inotify_init::InotifyInitSyscall,
            inotify_init1::InotifyInit1Syscall,
            inotify_add_watch::InotifyAddWatchSyscall,
            inotify_rm_watch::InotifyRmWatchSyscall,
            link::LinkSyscall,
            linkat::LinkAtSyscall,
            poll::PollSyscall,
            pselect6::Pselect6Syscall,
            ppoll::PpollSyscall,
            sendfile::SendfileSyscall,
            prctl::PrctlSyscall,
            epoll_create::EpollCreateSyscall,
            epoll_create::EpollCreate1Syscall,
            epoll_ctl::EpollCtlSyscall,
            epoll_wait::EpollWaitSyscall,
            epoll_pwait::EpollPwaitSyscall,
            newfstatat::NewFstatAtSyscall,
            prlimit64::Prlimit64Syscall,
            fsopen::FsopenSyscall,
            stat::StatSyscall,
            statx::StatxSyscall,
            statfs::StatfsSyscall,
            tgkill::TgkillSyscall,
            uname::UnameSyscall,
            unlink::UnlinkSyscall,
            unlinkat::UnlinkAtSyscall,
            rename::RenameSyscall,
            renameat::RenameAtSyscall,
            symlink::SymlinkSyscall,
            umask::UmaskSyscall,
            setuid::SetUidSyscall,
            setgid::SetGidSyscall,
            setgroups::SetGroupsSyscall,
            setpgid::SetPgidSyscall,
            setresuid::SetResUidSyscall,
            setresgid::SetResGidSyscall,
            writev::WritevSyscall,
        ]
    );
}

fn read_string_vector(
    ctx: &dyn KernelSyscallContext,
    pointers: &[u64],
) -> Result<Vec<String>, crate::errno::SysErr> {
    let mut values = Vec::with_capacity(pointers.len());
    for pointer in pointers {
        if *pointer == 0 {
            break;
        }
        values.push(ctx.read_user_c_string(*pointer, 512)?);
    }
    Ok(values)
}
