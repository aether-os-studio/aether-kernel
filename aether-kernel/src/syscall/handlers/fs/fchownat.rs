use crate::arch::syscall::nr;
use crate::credentials::Credentials;
use crate::errno::{SysErr, SysResult};
use crate::process::{ProcessServices, ProcessSyscallContext};
use crate::syscall::SyscallDisposition;
use crate::syscall::abi::{arg_i64_from_i32, read_path_allow_empty};

crate::declare_syscall!(
    pub struct FchownAtSyscall => nr::FCHOWNAT, "fchownat", |ctx, args| {
        let dirfd = arg_i64_from_i32(args.get(0));
        let Ok(path) = read_path_allow_empty(ctx, args.get(1), args.get(4), 4096) else {
            return SyscallDisposition::Return(Err(crate::errno::SysErr::Fault));
        };
        SyscallDisposition::Return(ctx.fchownat(
            dirfd,
            &path,
            args.get(2),
            args.get(3),
            args.get(4),
        ))
    }
);

impl<S: ProcessServices> ProcessSyscallContext<'_, S> {
    pub(crate) fn read_optional_chown_id(raw: u64) -> SysResult<Option<u32>> {
        let value = u32::try_from(raw).map_err(|_| SysErr::Inval)?;
        Ok((value != u32::MAX).then_some(value))
    }

    fn caller_in_group(credentials: &Credentials, gid: u32) -> bool {
        gid == credentials.gid
            || gid == credentials.egid
            || gid == credentials.sgid
            || gid == credentials.fsgid
            || credentials.supplementary_groups.contains(&gid)
    }

    pub(crate) fn may_chown(
        &self,
        current_uid: u32,
        current_gid: u32,
        new_uid: Option<u32>,
        new_gid: Option<u32>,
    ) -> SysResult<()> {
        let credentials = &self.process.credentials;
        if credentials.is_superuser() {
            return Ok(());
        }

        if credentials.fsuid != current_uid {
            return Err(SysErr::Perm);
        }

        if let Some(uid) = new_uid
            && uid != current_uid
        {
            return Err(SysErr::Perm);
        }

        if let Some(gid) = new_gid
            && gid != current_gid
            && !Self::caller_in_group(credentials, gid)
        {
            return Err(SysErr::Perm);
        }

        Ok(())
    }

    pub(crate) fn syscall_fchownat(
        &mut self,
        dirfd: i64,
        path: &str,
        owner: u64,
        group: u64,
        flags: u64,
    ) -> SysResult<u64> {
        const AT_SYMLINK_NOFOLLOW: u64 = crate::syscall::abi::AT_SYMLINK_NOFOLLOW;
        const AT_EMPTY_PATH: u64 = crate::syscall::abi::AT_EMPTY_PATH;
        const VALID_FLAGS: u64 = AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH;

        if (flags & !VALID_FLAGS) != 0 {
            return Err(SysErr::Inval);
        }
        if path.is_empty() && (flags & AT_EMPTY_PATH) == 0 {
            return Err(SysErr::NoEnt);
        }

        let owner = Self::read_optional_chown_id(owner)?;
        let group = Self::read_optional_chown_id(group)?;

        let node = if path.is_empty() {
            let descriptor = self.process.files.get(dirfd as u32).ok_or(SysErr::BadFd)?;
            descriptor.file.lock().node()
        } else {
            let fs_view = self.fs_view_for_dirfd(dirfd, path)?;
            let (node, _) = self.services.lookup_node_with_identity(
                &fs_view,
                path,
                (flags & AT_SYMLINK_NOFOLLOW) == 0,
            )?;
            node
        };

        let metadata = node.metadata();
        self.may_chown(metadata.uid, metadata.gid, owner, group)?;

        let next_uid = owner.unwrap_or(metadata.uid);
        let next_gid = group.unwrap_or(metadata.gid);
        if next_uid == metadata.uid && next_gid == metadata.gid {
            return Ok(0);
        }

        node.set_owner(next_uid, next_gid).map_err(SysErr::from)?;
        crate::fs::notify_attrib(&node);
        Ok(0)
    }
}
