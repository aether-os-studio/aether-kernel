extern crate alloc;

use aether_vfs::{FileOperations, FsResult};

use super::inode::ExtInodeNode;
use super::io::{block_on_future, map_ext_error};

impl FileOperations for ExtInodeNode {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn open(&self) {
        if self.kind != aether_vfs::NodeKind::File {
            return;
        }

        let _guard = self.lock_io();
        let mut state = self.open_state.lock();
        if state.refs == 0
            && let Ok(file) = self.open_file()
        {
            state.file = Some(file);
        }
        state.refs = state.refs.saturating_add(1);
    }

    fn release(&self) {
        if self.kind != aether_vfs::NodeKind::File {
            return;
        }

        let _guard = self.lock_io();
        let mut state = self.open_state.lock();
        if state.refs == 0 {
            return;
        }
        state.refs -= 1;
        if state.refs == 0 {
            state.file = None;
        }
    }

    fn read(&self, offset: usize, buffer: &mut [u8]) -> FsResult<usize> {
        self.with_open_file(|file| {
            block_on_future(file.read_bytes_at(buffer, offset as u64)).map_err(map_ext_error)
        })
    }

    fn size(&self) -> usize {
        self.metadata.lock().size.min(usize::MAX as u64) as usize
    }

    fn write(&self, offset: usize, buffer: &[u8]) -> FsResult<usize> {
        self.with_open_file(|file| {
            block_on_future(file.write_bytes_at(buffer, offset as u64)).map_err(map_ext_error)
        })
    }

    fn truncate(&self, size: usize) -> FsResult<()> {
        self.with_open_file(|file| {
            block_on_future(file.truncate(size as u64)).map_err(map_ext_error)
        })
    }
}
