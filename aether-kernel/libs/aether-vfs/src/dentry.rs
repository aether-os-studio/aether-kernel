extern crate alloc;

use alloc::string::String;
use alloc::sync::{Arc, Weak};

use aether_frame::libs::spin::SpinLock;

use crate::NodeRef;

pub type DentryRef = Arc<Dentry>;

pub struct Dentry {
    name: String,
    inode: NodeRef,
    parent: SpinLock<Option<Weak<Dentry>>>,
}

impl Dentry {
    pub fn new_root(inode: NodeRef) -> DentryRef {
        Arc::new(Self {
            name: String::from("/"),
            inode,
            parent: SpinLock::new(None),
        })
    }

    pub fn new(name: impl Into<String>, inode: NodeRef, parent: Option<&DentryRef>) -> DentryRef {
        Arc::new(Self {
            name: name.into(),
            inode,
            parent: SpinLock::new(parent.map(Arc::downgrade)),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn inode(&self) -> NodeRef {
        self.inode.clone()
    }

    pub fn parent(&self) -> Option<DentryRef> {
        self.parent.lock_irqsave().as_ref().and_then(Weak::upgrade)
    }

    pub fn absolute_path(&self) -> String {
        let mut parts = alloc::vec::Vec::new();
        let mut current = self.parent();
        if self.name != "/" {
            parts.push(self.name.clone());
        }
        while let Some(dentry) = current {
            if dentry.name != "/" {
                parts.push(dentry.name.clone());
            }
            current = dentry.parent();
        }
        parts.reverse();
        if parts.is_empty() {
            String::from("/")
        } else {
            alloc::format!("/{}", parts.join("/"))
        }
    }

    pub fn lookup(parent: &DentryRef, name: &str) -> Option<DentryRef> {
        let inode = parent.inode.lookup(name)?;
        Some(Self::new(name, inode, Some(parent)))
    }

    pub fn create_file(parent: &DentryRef, name: &str, mode: u32) -> crate::FsResult<DentryRef> {
        let inode = parent.inode.create_file(String::from(name), mode)?;
        Ok(Self::new(name, inode, Some(parent)))
    }

    pub fn create_dir(parent: &DentryRef, name: &str, mode: u32) -> crate::FsResult<DentryRef> {
        let inode = parent.inode.create_dir(String::from(name), mode)?;
        Ok(Self::new(name, inode, Some(parent)))
    }
}
