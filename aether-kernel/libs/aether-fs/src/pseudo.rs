extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

use aether_vfs::{DirectoryNode, FileNode, FileOperations, FsResult, NodeRef};

pub type BytesGenerator = Arc<dyn Fn() -> Vec<u8> + Send + Sync>;
pub type TextGenerator = Arc<dyn Fn() -> String + Send + Sync>;

pub fn directory(name: impl Into<String>) -> NodeRef {
    DirectoryNode::new(name)
}

pub fn directory_with_mode(name: impl Into<String>, mode: u32) -> NodeRef {
    DirectoryNode::new_with_mode(name, mode)
}

pub fn generated_bytes_file(
    name: impl Into<String>,
    mode: u32,
    generator: BytesGenerator,
) -> NodeRef {
    FileNode::new_with_mode(name, mode, 0, Arc::new(GeneratedBytesFile { generator }))
}

pub fn generated_text_file(
    name: impl Into<String>,
    mode: u32,
    generator: TextGenerator,
) -> NodeRef {
    generated_bytes_file(name, mode, Arc::new(move || generator().into_bytes()))
}

struct GeneratedBytesFile {
    generator: BytesGenerator,
}

impl FileOperations for GeneratedBytesFile {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read(&self, offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        let bytes = (self.generator)();
        if offset >= bytes.len() {
            return Ok(0);
        }

        let len = core::cmp::min(buffer.len(), bytes.len() - offset);
        buffer[..len].copy_from_slice(&bytes[offset..offset + len]);
        Ok(len)
    }

    fn size(&self) -> usize {
        (self.generator)().len()
    }
}
