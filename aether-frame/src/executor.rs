extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::future::Future;
use core::mem::ManuallyDrop;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use crate::libs::spin::SpinLock;

type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

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
        let waker = task_waker(self.clone());
        let mut context = Context::from_waker(&waker);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(()) => {}
            Poll::Pending => {
                *future_slot = Some(future);
            }
        }
    }
}

struct WaitParker {
    woken: AtomicBool,
}

impl WaitParker {
    const fn new() -> Self {
        Self {
            woken: AtomicBool::new(false),
        }
    }

    fn take_wake(&self) -> bool {
        self.woken.swap(false, Ordering::AcqRel)
    }
}

static READY_QUEUE: SpinLock<VecDeque<Arc<Task>>> = SpinLock::new(VecDeque::new());

pub fn spawn(future: impl Future<Output = ()> + Send + 'static) {
    enqueue_task(Task::new(Box::pin(future)));
}

pub fn run_ready() {
    loop {
        let task = READY_QUEUE.lock_irqsave().pop_front();
        let Some(task) = task else {
            break;
        };
        task.poll();
    }
}

pub fn block_on<F>(future: F) -> F::Output
where
    F: Future,
{
    let parker = Arc::new(WaitParker::new());
    let waker = parker_waker(parker.clone());
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);

    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }

        run_ready();
        if !crate::interrupt::are_enabled() {
            continue;
        }
        if !parker.take_wake() {
            crate::arch::cpu::wait_for_interrupt();
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
    READY_QUEUE.lock_irqsave().push_back(task);
}

fn task_waker(task: Arc<Task>) -> Waker {
    unsafe { Waker::from_raw(task_raw_waker(task)) }
}

fn parker_waker(parker: Arc<WaitParker>) -> Waker {
    unsafe { Waker::from_raw(parker_raw_waker(parker)) }
}

fn task_raw_waker(task: Arc<Task>) -> RawWaker {
    RawWaker::new(Arc::into_raw(task).cast::<()>(), &TASK_WAKER_VTABLE)
}

fn parker_raw_waker(parker: Arc<WaitParker>) -> RawWaker {
    RawWaker::new(Arc::into_raw(parker).cast::<()>(), &PARKER_WAKER_VTABLE)
}

unsafe fn task_clone(data: *const ()) -> RawWaker {
    let task = ManuallyDrop::new(Arc::from_raw(data.cast::<Task>()));
    task_raw_waker(Arc::clone(&task))
}

unsafe fn task_wake(data: *const ()) {
    enqueue_task(Arc::from_raw(data.cast::<Task>()));
}

unsafe fn task_wake_by_ref(data: *const ()) {
    let task = ManuallyDrop::new(Arc::from_raw(data.cast::<Task>()));
    enqueue_task(Arc::clone(&task));
}

unsafe fn task_drop(data: *const ()) {
    drop(Arc::from_raw(data.cast::<Task>()));
}

unsafe fn parker_clone(data: *const ()) -> RawWaker {
    let parker = ManuallyDrop::new(Arc::from_raw(data.cast::<WaitParker>()));
    parker_raw_waker(Arc::clone(&parker))
}

unsafe fn parker_wake(data: *const ()) {
    let parker = Arc::from_raw(data.cast::<WaitParker>());
    parker.woken.store(true, Ordering::Release);
}

unsafe fn parker_wake_by_ref(data: *const ()) {
    let parker = ManuallyDrop::new(Arc::from_raw(data.cast::<WaitParker>()));
    parker.woken.store(true, Ordering::Release);
}

unsafe fn parker_drop(data: *const ()) {
    drop(Arc::from_raw(data.cast::<WaitParker>()));
}

static TASK_WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(task_clone, task_wake, task_wake_by_ref, task_drop);
static PARKER_WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(parker_clone, parker_wake, parker_wake_by_ref, parker_drop);
