#![no_std]

extern crate alloc;

mod block;
pub mod ext;
mod pipe;
pub mod pseudo;
mod runtime_async;

pub(crate) use self::block::TransferBufferLease;
pub use self::block::{
    AsyncBlockDevice, BlockDeviceFile, BlockFuture, BlockGeometry, SyncBlockDevice,
    SyncToAsyncBlockDevice,
};
pub use self::pipe::anonymous_pipe;

pub(crate) use self::runtime_async::block_on_future;
