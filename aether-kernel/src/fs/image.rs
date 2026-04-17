extern crate alloc;

use aether_process::{ImageError, ProgramImageSource};
use aether_vfs::NodeRef;

pub struct NodeImageSource {
    node: NodeRef,
    len: usize,
}

impl NodeImageSource {
    pub fn new(node: NodeRef) -> Option<Self> {
        let len = node.file()?.size();
        node.open();
        Some(Self { node, len })
    }
}

impl Clone for NodeImageSource {
    fn clone(&self) -> Self {
        self.node.open();
        Self {
            node: self.node.clone(),
            len: self.len,
        }
    }
}

impl Drop for NodeImageSource {
    fn drop(&mut self) {
        self.node.release();
    }
}

impl ProgramImageSource for NodeImageSource {
    fn len(&self) -> usize {
        self.len
    }

    fn read_exact_at(&self, offset: usize, buffer: &mut [u8]) -> Result<(), ImageError> {
        let mut filled = 0usize;
        while filled < buffer.len() {
            let read = self
                .node
                .read(offset + filled, &mut buffer[filled..])
                .map_err(|_| ImageError::ReadFailure)?;
            if read == 0 {
                return Err(ImageError::UnexpectedEof);
            }
            filled += read;
        }
        Ok(())
    }
}
