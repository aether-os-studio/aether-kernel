#![no_std]

extern crate alloc;

use aether_vfs::{CowMemoryFile, DirectoryNode, FileNode, MutableMemoryFile, NodeRef, SymlinkNode};

pub fn directory(name: impl Into<alloc::string::String>) -> NodeRef {
    DirectoryNode::new(name)
}

pub fn directory_with_mode(name: impl Into<alloc::string::String>, mode: u32) -> NodeRef {
    DirectoryNode::new_with_mode(name, mode)
}

pub fn file(name: impl Into<alloc::string::String>, bytes: &[u8]) -> NodeRef {
    FileNode::new(name, alloc::sync::Arc::new(MutableMemoryFile::new(bytes)))
}

pub fn file_with_mode(name: impl Into<alloc::string::String>, bytes: &[u8], mode: u32) -> NodeRef {
    FileNode::new_with_mode(
        name,
        mode,
        0,
        alloc::sync::Arc::new(MutableMemoryFile::new(bytes)),
    )
}

pub fn borrowed_file_with_mode(
    name: impl Into<alloc::string::String>,
    bytes: &'static [u8],
    mode: u32,
) -> NodeRef {
    FileNode::new_with_mode(
        name,
        mode,
        0,
        alloc::sync::Arc::new(CowMemoryFile::new_borrowed(bytes)),
    )
}

pub fn symlink(
    name: impl Into<alloc::string::String>,
    target: impl Into<alloc::string::String>,
) -> NodeRef {
    SymlinkNode::new(name, target)
}

pub fn symlink_with_mode(
    name: impl Into<alloc::string::String>,
    target: impl Into<alloc::string::String>,
    mode: u32,
) -> NodeRef {
    SymlinkNode::new_with_mode(name, target, mode)
}
