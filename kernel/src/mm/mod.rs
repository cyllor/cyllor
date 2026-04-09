pub mod pmm;
mod heap;
pub mod mmap;
pub mod shm;
pub mod vmm;

use linked_list_allocator::LockedHeap;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

const KERNEL_HEAP_SIZE: usize = 32 * 1024 * 1024; // 32 MiB

pub fn init() {
    pmm::init();

    // Find a contiguous block of physical memory for the heap
    // by scanning the memory map for a large enough USABLE region
    let hhdm = crate::arch::hhdm_offset();
    let heap_phys = find_contiguous_region(KERNEL_HEAP_SIZE)
        .expect("Cannot find contiguous region for kernel heap");

    // Mark these pages as used in PMM
    let num_pages = KERNEL_HEAP_SIZE / 4096;
    // Pages are already marked used since PMM allocates from them
    // We need to explicitly allocate them to prevent reuse
    for i in 0..num_pages {
        let page = heap_phys + i * 4096;
        pmm::mark_used(page);
    }

    let heap_start = heap_phys as u64 + hhdm;

    // Zero and init
    unsafe {
        core::ptr::write_bytes(heap_start as *mut u8, 0, KERNEL_HEAP_SIZE);
        ALLOCATOR.lock().init(heap_start as *mut u8, KERNEL_HEAP_SIZE);
    }

    log::debug!("Kernel heap: {} MiB at phys 0x{:x} (HHDM 0x{:x})",
        KERNEL_HEAP_SIZE / (1024*1024), heap_phys, heap_start);

    mmap::init();
}

/// Find a contiguous region of usable physical memory of at least `size` bytes
fn find_contiguous_region(size: usize) -> Option<usize> {
    let entries = crate::arch::memory_map();

    for entry in entries {
        if entry.type_ == limine::memmap::MEMMAP_USABLE {
            let base = entry.base as usize;
            let len = entry.length as usize;
            // Skip the first 64MB to avoid conflicts with early PMM allocations
            let safe_base = base.max(0x44000000);
            let safe_len = if safe_base >= base + len { 0 } else { base + len - safe_base };
            // Align to 2MB for clean mapping
            let aligned_base = (safe_base + 0x1FFFFF) & !0x1FFFFF;
            let aligned_len = if aligned_base >= safe_base + safe_len { 0 } else { safe_base + safe_len - aligned_base };

            if aligned_len >= size {
                return Some(aligned_base);
            }
        }
    }
    None
}

pub use pmm::{alloc_page, free_page};
pub use mmap::{do_brk, do_mmap, do_mprotect, do_munmap};
pub use shm::{do_shmat, do_shmctl, do_shmdt, do_shmget};
