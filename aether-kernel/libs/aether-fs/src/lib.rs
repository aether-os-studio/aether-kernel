#![no_std]

extern crate alloc;

mod block;
pub mod ext;
mod pipe;
pub mod pseudo;

pub use self::block::{
    AsyncBlockDevice, BlockDeviceFile, BlockFuture, BlockGeometry, SyncBlockDevice,
    SyncToAsyncBlockDevice,
};
pub use self::pipe::anonymous_pipe;
