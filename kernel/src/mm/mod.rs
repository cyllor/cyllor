pub mod pmm;
mod heap;
mod mmap;

use core::cell::UnsafeCell;
use linked_list_allocator::LockedHeap;
use crate::syscall::{SyscallResult, ENOMEM, EINVAL};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

const KERNEL_HEAP_SIZE: usize = 64 * 1024 * 1024; // 64 MiB

#[repr(align(4096))]
struct HeapSpace(UnsafeCell<[u8; KERNEL_HEAP_SIZE]>);
unsafe impl Sync for HeapSpace {}

static HEAP_SPACE: HeapSpace = HeapSpace(UnsafeCell::new([0; KERNEL_HEAP_SIZE]));

pub fn init() {
    pmm::init();

    unsafe {
        let heap_start = HEAP_SPACE.0.get() as *mut u8;
        ALLOCATOR.lock().init(heap_start, KERNEL_HEAP_SIZE);
    }
    log::debug!("Kernel heap: {} KiB", KERNEL_HEAP_SIZE / 1024);

    mmap::init();
}

pub use pmm::{alloc_page, free_page};
pub use mmap::{do_mmap, do_munmap, do_brk};
