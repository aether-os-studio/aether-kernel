extern crate alloc;

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;

use aether_fs::pseudo::generated_text_file;
use aether_vfs::{DirectoryEntry, Inode, InodeOperations, NodeKind, NodeRef};

use crate::process::{Pid, ProcessState};
use crate::runtime::KernelRuntime;

pub fn lookup_virtual(path: &str) -> Option<crate::errno::SysResult<NodeRef>> {
    if path == "/proc/self" {
        return Some(
            current_pid()
                .ok_or(crate::errno::SysErr::NoEnt)
                .map(|pid| proc_pid_dir("self", pid)),
        );
    }
    if path == "/proc/self/status" {
        return Some(
            current_pid()
                .ok_or(crate::errno::SysErr::NoEnt)
                .map(proc_status_file),
        );
    }
    if path == "/proc/self/comm" {
        return Some(
            current_pid()
                .ok_or(crate::errno::SysErr::NoEnt)
                .map(proc_comm_file),
        );
    }

    let remainder = path.strip_prefix("/proc/")?;
    let mut components = remainder.split('/');
    let pid = components.next()?.parse::<u32>().ok()?;
    match components.next() {
        None => Some(Ok(proc_pid_dir(pid.to_string().as_str(), pid))),
        Some("status") if components.next().is_none() => Some(Ok(proc_status_file(pid))),
        Some("comm") if components.next().is_none() => Some(Ok(proc_comm_file(pid))),
        _ => Some(Err(crate::errno::SysErr::NoEnt)),
    }
}

fn current_pid() -> Option<Pid> {
    KernelRuntime::current_pid()
}

fn proc_pid_dir(name: &str, pid: Pid) -> NodeRef {
    Inode::new(Arc::new(ProcPidDirectory {
        name: name.to_string(),
        pid,
    }))
}

fn proc_status_file(pid: Pid) -> NodeRef {
    generated_text_file(
        "status",
        0o100444,
        Arc::new(move || render_status(pid).unwrap_or_default()),
    )
}

fn proc_comm_file(pid: Pid) -> NodeRef {
    generated_text_file(
        "comm",
        0o100444,
        Arc::new(move || {
            snapshot_name(pid)
                .map(|name| alloc::format!("{name}\n"))
                .unwrap_or_default()
        }),
    )
}

fn snapshot_name(pid: Pid) -> Option<String> {
    KernelRuntime::procfs_snapshot(pid).map(|snapshot| process_name(&snapshot.name))
}

fn render_status(pid: Pid) -> Option<String> {
    let snapshot = KernelRuntime::procfs_snapshot(pid)?;
    let name = process_name(&snapshot.name);
    let state = linux_state(snapshot.state);
    let ppid = snapshot.parent.unwrap_or(0);
    let groups = if snapshot.credentials.supplementary_groups.is_empty() {
        String::new()
    } else {
        snapshot
            .credentials
            .supplementary_groups
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    };

    Some(alloc::format!(
        "Name:\t{name}\nState:\t{state}\nTgid:\t{pid}\nPid:\t{pid}\nPPid:\t{ppid}\nUid:\t{uid}\t{euid}\t{suid}\t{fsuid}\nGid:\t{gid}\t{egid}\t{sgid}\t{fsgid}\nGroups:\t{groups}\nUmask:\t{umask:04o}\n",
        uid = snapshot.credentials.uid,
        euid = snapshot.credentials.euid,
        suid = snapshot.credentials.suid,
        fsuid = snapshot.credentials.fsuid,
        gid = snapshot.credentials.gid,
        egid = snapshot.credentials.egid,
        sgid = snapshot.credentials.sgid,
        fsgid = snapshot.credentials.fsgid,
        umask = snapshot.umask,
    ))
}

fn linux_state(state: ProcessState) -> &'static str {
    match state {
        ProcessState::Running => "R (running)",
        ProcessState::Runnable => "R (running)",
        ProcessState::Blocked(_) => "S (sleeping)",
        ProcessState::Stopped(_) => "T (stopped)",
        ProcessState::Exited(_) => "Z (zombie)",
        ProcessState::Faulted { .. } => "D (disk sleep)",
    }
}

fn process_name(name: &[u8; 16]) -> String {
    let len = name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name.len());
    String::from(core::str::from_utf8(&name[..len]).unwrap_or("unknown"))
}

struct ProcPidDirectory {
    name: String,
    pid: Pid,
}

impl InodeOperations for ProcPidDirectory {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> NodeKind {
        NodeKind::Directory
    }

    fn lookup(&self, name: &str) -> Option<NodeRef> {
        match name {
            "status" => Some(proc_status_file(self.pid)),
            "comm" => Some(proc_comm_file(self.pid)),
            _ => None,
        }
    }

    fn entries(&self) -> Vec<DirectoryEntry> {
        vec![
            DirectoryEntry {
                name: String::from("status"),
                kind: NodeKind::File,
            },
            DirectoryEntry {
                name: String::from("comm"),
                kind: NodeKind::File,
            },
        ]
    }

    fn mode(&self) -> Option<u32> {
        Some(0o040555)
    }
}
