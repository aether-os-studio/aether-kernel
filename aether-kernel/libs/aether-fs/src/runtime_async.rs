use core::future::Future;

struct FrameParker;

impl aether_async::Parker for FrameParker {
    fn can_park(&self) -> bool {
        aether_frame::interrupt::are_enabled()
    }

    fn park(&self) {
        aether_frame::arch::cpu::wait_for_interrupt();
    }
}

pub(crate) fn block_on_future<F>(future: F) -> F::Output
where
    F: Future,
{
    aether_async::block_on_with(future, &FrameParker)
}
