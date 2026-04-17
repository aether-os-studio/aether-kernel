#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::task::Wake;
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, Waker};

use aether_frame::libs::spin::SpinLock;

type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

pub trait Parker {
    fn can_park(&self) -> bool;
    fn park(&self);
}

struct Task {
    queued: AtomicBool,
    future: SpinLock<Option<BoxFuture>>,
}

impl Task {
    fn new(future: BoxFuture) -> Arc<Self> {
        Arc::new(Self {
            queued: AtomicBool::new(false),
            future: SpinLock::new(Some(future)),
        })
    }

    fn poll(self: Arc<Self>) {
        let mut future_slot = self.future.lock();
        let Some(mut future) = future_slot.take() else {
            self.queued.store(false, Ordering::Release);
            return;
        };

        self.queued.store(false, Ordering::Release);
        let waker = Waker::from(self.clone());
        let mut context = Context::from_waker(&waker);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(()) => {}
            Poll::Pending => {
                *future_slot = Some(future);
            }
        }
    }
}

impl Wake for Task {
    fn wake(self: Arc<Self>) {
        enqueue_task(self);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        enqueue_task(self.clone());
    }
}

struct WaitNotifier {
    woken: AtomicBool,
}

impl WaitNotifier {
    const fn new() -> Self {
        Self {
            woken: AtomicBool::new(false),
        }
    }

    fn take_wake(&self) -> bool {
        self.woken.swap(false, Ordering::AcqRel)
    }
}

impl Wake for WaitNotifier {
    fn wake(self: Arc<Self>) {
        self.woken.store(true, Ordering::Release);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::Release);
    }
}

static READY_QUEUE: SpinLock<VecDeque<Arc<Task>>> = SpinLock::new(VecDeque::new());

pub fn spawn(future: impl Future<Output = ()> + Send + 'static) {
    enqueue_task(Task::new(Box::pin(future)));
}

pub fn run_ready() {
    loop {
        let task = READY_QUEUE.lock().pop_front();
        let Some(task) = task else {
            break;
        };
        task.poll();
    }
}

pub fn block_on_with<F, P>(future: F, parker: &P) -> F::Output
where
    F: Future,
    P: Parker + ?Sized,
{
    let notifier = Arc::new(WaitNotifier::new());
    let waker = Waker::from(notifier.clone());
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);

    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }

        run_ready();
        if !parker.can_park() {
            continue;
        }
        if !notifier.take_wake() {
            parker.park();
        }
    }
}

fn enqueue_task(task: Arc<Task>) {
    if task
        .queued
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    READY_QUEUE.lock().push_back(task);
}
