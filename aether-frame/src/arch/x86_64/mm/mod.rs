mod paging;

use crate::boot::phys_to_virt;
use crate::mm::{AddressSpace, PageTableArch, PhysFrame, frame_allocator};

pub use self::paging::{ArchitecturePageTable, PageTableEntry};

pub fn new_user_root() -> Result<PhysFrame, crate::mm::MappingError> {
    let mut allocator = frame_allocator().lock_irqsave();
    let root = AddressSpace::<ArchitecturePageTable>::new_root(&mut *allocator)?.root();
    copy_kernel_pml4_half(root);
    Ok(root)
}

fn copy_kernel_pml4_half(root: PhysFrame) {
    let current = ArchitecturePageTable::root_frame();
    let current_ptr = phys_to_virt(current.start_address().as_u64()) as *const u64;
    let root_ptr = phys_to_virt(root.start_address().as_u64()) as *mut u64;

    for index in 256..512 {
        unsafe {
            root_ptr.add(index).write(current_ptr.add(index).read());
        }
    }
}
