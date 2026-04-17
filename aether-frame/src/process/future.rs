use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use crate::arch::process::Process;

use super::RunResult;

pub struct RunFuture<'a> {
    process: &'a mut Process,
}

impl<'a> RunFuture<'a> {
    pub(crate) const fn new(process: &'a mut Process) -> Self {
        Self { process }
    }
}

impl Future for RunFuture<'_> {
    type Output = RunResult;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(self.process.run())
    }
}
