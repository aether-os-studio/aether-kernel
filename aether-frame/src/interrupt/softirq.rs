use core::sync::atomic::{AtomicUsize, Ordering};

pub type SoftIrqHandler = fn();

static PENDING: AtomicUsize = AtomicUsize::new(0);
static mut HANDLERS: [Option<SoftIrqHandler>; usize::BITS as usize] = [None; usize::BITS as usize];

pub fn register(id: usize, handler: SoftIrqHandler) -> bool {
    if id >= usize::BITS as usize {
        return false;
    }
    unsafe {
        HANDLERS[id] = Some(handler);
    }
    true
}

pub fn raise(id: usize) -> bool {
    if id >= usize::BITS as usize {
        return false;
    }
    PENDING.fetch_or(1usize << id, Ordering::AcqRel);
    true
}

pub fn drain_pending() {
    let pending = PENDING.swap(0, Ordering::AcqRel);
    let mut bit = 0usize;
    while bit < usize::BITS as usize {
        if (pending & (1usize << bit)) == 0 {
            bit += 1;
            continue;
        }

        if let Some(handler) = unsafe { HANDLERS[bit] } {
            handler();
        }
        bit += 1;
    }
}
