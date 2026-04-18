extern crate alloc;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};

use aether_frame::libs::spin::SpinLock;
use aether_vfs::{
    FileOperations, FsError, FsResult, NodeKind, NodeRef, PollEvents, SharedWaitListener, WaitQueue,
};

pub const IN_ACCESS: u32 = 0x0000_0001;
pub const IN_MODIFY: u32 = 0x0000_0002;
pub const IN_ATTRIB: u32 = 0x0000_0004;
pub const IN_CLOSE_WRITE: u32 = 0x0000_0008;
pub const IN_CLOSE_NOWRITE: u32 = 0x0000_0010;
pub const IN_OPEN: u32 = 0x0000_0020;
pub const IN_MOVED_FROM: u32 = 0x0000_0040;
pub const IN_MOVED_TO: u32 = 0x0000_0080;
pub const IN_CREATE: u32 = 0x0000_0100;
pub const IN_DELETE: u32 = 0x0000_0200;
pub const IN_DELETE_SELF: u32 = 0x0000_0400;
pub const IN_MOVE_SELF: u32 = 0x0000_0800;
pub const IN_UNMOUNT: u32 = 0x0000_2000;
#[allow(dead_code)]
pub const IN_Q_OVERFLOW: u32 = 0x0000_4000;
pub const IN_IGNORED: u32 = 0x0000_8000;
pub const IN_ONLYDIR: u32 = 0x0100_0000;
pub const IN_DONT_FOLLOW: u32 = 0x0200_0000;
pub const IN_EXCL_UNLINK: u32 = 0x0400_0000;
pub const IN_MASK_CREATE: u32 = 0x1000_0000;
pub const IN_MASK_ADD: u32 = 0x2000_0000;
pub const IN_ISDIR: u32 = 0x4000_0000;
pub const IN_ONESHOT: u32 = 0x8000_0000;

pub const IN_ALL_EVENTS: u32 = IN_ACCESS
    | IN_MODIFY
    | IN_ATTRIB
    | IN_CLOSE_WRITE
    | IN_CLOSE_NOWRITE
    | IN_OPEN
    | IN_MOVED_FROM
    | IN_MOVED_TO
    | IN_CREATE
    | IN_DELETE
    | IN_DELETE_SELF
    | IN_MOVE_SELF;

pub const INOTIFY_INIT1_VALID_FLAGS: u64 = 0o0004000 | 0o2000000;
pub const INOTIFY_ADD_WATCH_VALID_MASK: u32 = IN_ALL_EVENTS
    | IN_UNMOUNT
    | IN_ONLYDIR
    | IN_DONT_FOLLOW
    | IN_EXCL_UNLINK
    | IN_MASK_CREATE
    | IN_MASK_ADD
    | IN_ONESHOT;

static NEXT_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_COOKIE: AtomicU32 = AtomicU32::new(1);
static REGISTRY: SpinLock<InotifyRegistry> = SpinLock::new(InotifyRegistry::new());

pub fn create_inotify_instance() -> Arc<InotifyFile> {
    let instance = Arc::new(InotifyFile::new());
    REGISTRY
        .lock()
        .instances
        .insert(instance.instance_id, Arc::downgrade(&instance));
    instance
}

pub fn notify_attrib(node: &NodeRef) {
    dispatch_target_event(node, IN_ATTRIB, 0, None, false, false);
}

pub fn notify_create(parent: &NodeRef, child: &NodeRef, name: &str) {
    dispatch_target_event(
        parent,
        IN_CREATE,
        0,
        Some(name),
        child.kind() == NodeKind::Directory,
        false,
    );
}

pub fn notify_delete(parent: &NodeRef, child: &NodeRef, name: &str) {
    dispatch_target_event(
        parent,
        IN_DELETE,
        0,
        Some(name),
        child.kind() == NodeKind::Directory,
        true,
    );
    dispatch_target_event(child, IN_DELETE_SELF, 0, None, false, true);
}

pub fn notify_move(
    old_parent: &NodeRef,
    new_parent: &NodeRef,
    node: &NodeRef,
    old_name: &str,
    new_name: &str,
) {
    let cookie = NEXT_COOKIE.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    let is_dir = node.kind() == NodeKind::Directory;
    dispatch_target_event(
        old_parent,
        IN_MOVED_FROM,
        cookie,
        Some(old_name),
        is_dir,
        false,
    );
    dispatch_target_event(
        new_parent,
        IN_MOVED_TO,
        cookie,
        Some(new_name),
        is_dir,
        false,
    );
    dispatch_target_event(node, IN_MOVE_SELF, 0, None, false, false);
}

pub struct InotifyFile {
    instance_id: u64,
    next_watch_descriptor: AtomicI32,
    watches: SpinLock<BTreeMap<i32, InotifyWatch>>,
    events: SpinLock<VecDeque<Vec<u8>>>,
    waiters: WaitQueue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct WatchTarget {
    device_id: u64,
    inode: u64,
}

#[derive(Clone, Copy, Debug)]
struct InotifyWatch {
    target: WatchTarget,
    mask: u32,
}

#[derive(Clone)]
struct WatchRegistration {
    instance_id: u64,
    wd: i32,
}

struct InotifyRegistry {
    instances: BTreeMap<u64, Weak<InotifyFile>>,
    targets: BTreeMap<WatchTarget, Vec<WatchRegistration>>,
}

impl InotifyRegistry {
    const fn new() -> Self {
        Self {
            instances: BTreeMap::new(),
            targets: BTreeMap::new(),
        }
    }

    fn register(&mut self, target: WatchTarget, registration: WatchRegistration) {
        self.targets.entry(target).or_default().push(registration);
    }

    fn unregister(&mut self, target: WatchTarget, instance_id: u64, wd: i32) {
        let Some(entries) = self.targets.get_mut(&target) else {
            return;
        };
        let instances = &self.instances;
        entries.retain(|registration| {
            !(registration.instance_id == instance_id && registration.wd == wd)
                && instances
                    .get(&registration.instance_id)
                    .and_then(Weak::upgrade)
                    .is_some()
        });
        if entries.is_empty() {
            self.targets.remove(&target);
        }
    }

    fn registrations(&mut self, target: WatchTarget) -> Vec<WatchRegistration> {
        let Some(entries) = self.targets.get_mut(&target) else {
            return Vec::new();
        };
        let instances = &self.instances;
        entries.retain(|registration| {
            instances
                .get(&registration.instance_id)
                .and_then(Weak::upgrade)
                .is_some()
        });
        entries.clone()
    }

    fn remove_target(&mut self, target: WatchTarget) -> Vec<WatchRegistration> {
        self.targets.remove(&target).unwrap_or_default()
    }
}

impl InotifyFile {
    fn new() -> Self {
        Self {
            instance_id: NEXT_INSTANCE_ID.fetch_add(1, Ordering::AcqRel),
            next_watch_descriptor: AtomicI32::new(1),
            watches: SpinLock::new(BTreeMap::new()),
            events: SpinLock::new(VecDeque::new()),
            waiters: WaitQueue::new(),
        }
    }

    pub fn add_watch(&self, node: &NodeRef, mask: u32) -> FsResult<i32> {
        let target = watch_target(node);
        let normalized_mask = mask & !IN_MASK_ADD & !IN_MASK_CREATE;
        let mut watches = self.watches.lock();
        if let Some((wd, watch)) = watches
            .iter_mut()
            .find(|(_, watch)| watch.target == target)
            .map(|(wd, watch)| (*wd, watch))
        {
            if (mask & IN_MASK_CREATE) != 0 {
                return Err(FsError::AlreadyExists);
            }
            if (mask & IN_MASK_ADD) != 0 {
                watch.mask |= normalized_mask;
            } else {
                watch.mask = normalized_mask;
            }
            return Ok(wd);
        }

        let wd = self.next_watch_descriptor.fetch_add(1, Ordering::AcqRel);
        watches.insert(
            wd,
            InotifyWatch {
                target,
                mask: normalized_mask,
            },
        );
        drop(watches);

        REGISTRY.lock().register(
            target,
            WatchRegistration {
                instance_id: self.instance_id,
                wd,
            },
        );
        Ok(wd)
    }

    pub fn remove_watch(&self, wd: i32) -> FsResult<()> {
        let target = self.remove_watch_registration(wd)?;
        self.push_event(encode_event(wd, IN_IGNORED, 0, None));
        REGISTRY.lock().unregister(target, self.instance_id, wd);
        Ok(())
    }

    fn remove_watch_registration(&self, wd: i32) -> FsResult<WatchTarget> {
        self.watches
            .lock()
            .remove(&wd)
            .map(|watch| watch.target)
            .ok_or(FsError::InvalidInput)
    }

    fn watch(&self, wd: i32) -> Option<InotifyWatch> {
        self.watches.lock().get(&wd).copied()
    }

    fn push_event(&self, event: Vec<u8>) {
        let should_notify = {
            let mut events = self.events.lock();
            let was_empty = events.is_empty();
            events.push_back(event);
            was_empty
        };
        if should_notify {
            self.waiters.notify(PollEvents::READ);
        }
    }

    fn auto_remove_watch(&self, wd: i32, target: WatchTarget) {
        let removed = self.watches.lock().remove(&wd);
        if removed.is_some() {
            REGISTRY.lock().unregister(target, self.instance_id, wd);
            self.push_event(encode_event(wd, IN_IGNORED, 0, None));
        }
    }
}

impl FileOperations for InotifyFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read(&self, _offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }

        let mut events = self.events.lock();
        let Some(first) = events.front() else {
            return Err(FsError::WouldBlock);
        };
        if buffer.len() < first.len() {
            return Err(FsError::InvalidInput);
        }

        let mut written = 0usize;
        while let Some(event) = events.front() {
            if (buffer.len() - written) < event.len() {
                break;
            }
            let event = events.pop_front().expect("inotify event exists");
            buffer[written..written + event.len()].copy_from_slice(event.as_slice());
            written += event.len();
        }
        Ok(written)
    }

    fn poll(&self, events: PollEvents) -> FsResult<PollEvents> {
        let ready = if events.contains(PollEvents::READ) && !self.events.lock().is_empty() {
            PollEvents::READ
        } else {
            PollEvents::empty()
        };
        Ok(ready)
    }

    fn register_waiter(
        &self,
        events: PollEvents,
        listener: SharedWaitListener,
    ) -> FsResult<Option<u64>> {
        Ok(Some(self.waiters.register(events, listener)))
    }

    fn unregister_waiter(&self, waiter_id: u64) -> FsResult<()> {
        let _ = self.waiters.unregister(waiter_id);
        Ok(())
    }
}

fn dispatch_target_event(
    node: &NodeRef,
    mask: u32,
    cookie: u32,
    name: Option<&str>,
    set_isdir: bool,
    remove_watch: bool,
) {
    let target = watch_target(node);
    let registrations = {
        let mut registry = REGISTRY.lock();
        if remove_watch {
            registry.remove_target(target)
        } else {
            registry.registrations(target)
        }
    };

    let mut oneshot = Vec::new();
    for registration in registrations {
        let Some(instance) = REGISTRY
            .lock()
            .instances
            .get(&registration.instance_id)
            .and_then(Weak::upgrade)
        else {
            continue;
        };
        let Some(watch) = instance.watch(registration.wd) else {
            continue;
        };
        if (watch.mask & mask) == 0 {
            if remove_watch {
                instance.auto_remove_watch(registration.wd, target);
            }
            continue;
        }

        let mut event_mask = mask;
        if set_isdir {
            event_mask |= IN_ISDIR;
        }
        instance.push_event(encode_event(registration.wd, event_mask, cookie, name));
        if remove_watch || (watch.mask & IN_ONESHOT) != 0 {
            oneshot.push((instance, registration.wd, target));
        }
    }

    for (instance, wd, target) in oneshot {
        instance.auto_remove_watch(wd, target);
    }
}

fn watch_target(node: &NodeRef) -> WatchTarget {
    let metadata = node.metadata();
    WatchTarget {
        device_id: metadata.device_id,
        inode: metadata.inode,
    }
}

fn encode_event(wd: i32, mask: u32, cookie: u32, name: Option<&str>) -> Vec<u8> {
    let raw_name = name.unwrap_or("").as_bytes();
    let name_len = if raw_name.is_empty() {
        0
    } else {
        raw_name.len() + 1
    };
    let padded_len = (name_len + 3) & !3;
    let mut bytes = vec![0u8; 16 + padded_len];
    bytes[0..4].copy_from_slice(&wd.to_ne_bytes());
    bytes[4..8].copy_from_slice(&mask.to_ne_bytes());
    bytes[8..12].copy_from_slice(&cookie.to_ne_bytes());
    bytes[12..16].copy_from_slice(&(name_len as u32).to_ne_bytes());
    if !raw_name.is_empty() {
        bytes[16..16 + raw_name.len()].copy_from_slice(raw_name);
    }
    bytes
}
