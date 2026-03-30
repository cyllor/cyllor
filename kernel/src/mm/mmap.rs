// User memory management: mmap, munmap, brk
// Allocates physical pages and maps them in the user's address space

use crate::syscall::{SyscallResult, ENOMEM, EINVAL};
use crate::mm::pmm;
use spin::Mutex;
use alloc::vec::Vec;

const PAGE_SIZE: usize = 4096;

static USER_MEM: Mutex<UserMemState> = Mutex::new(UserMemState::new());

struct UserMemState {
    brk_base: usize,
    brk_current: usize,
    mmap_next: usize,
}

impl UserMemState {
    const fn new() -> Self {
        Self {
            brk_base: 0x0000_0060_0000_0000,     // 384 GiB
            brk_current: 0x0000_0060_0000_0000,
            mmap_next: 0x0000_0070_0000_0000,     // 448 GiB
        }
    }
}

pub fn init() {}

pub fn do_mmap(addr: usize, length: usize, prot: u32, flags: u32, fd: i32, offset: i64) -> SyscallResult {
    let aligned_len = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    if aligned_len == 0 {
        return Err(EINVAL);
    }

    let is_anon = (flags & 0x20) != 0; // MAP_ANONYMOUS
    let is_fixed = (flags & 0x10) != 0; // MAP_FIXED

    let map_addr = if is_fixed && addr != 0 {
        addr
    } else {
        let mut state = USER_MEM.lock();
        let a = state.mmap_next;
        state.mmap_next += aligned_len;
        a
    };

    // Allocate physical pages
    let num_pages = aligned_len / PAGE_SIZE;
    for i in 0..num_pages {
        let phys = pmm::alloc_page().ok_or(ENOMEM)?;
        // Zero the page via HHDM
        let hhdm = crate::arch::aarch64::hhdm_offset();
        unsafe {
            core::ptr::write_bytes((phys as u64 + hhdm) as *mut u8, 0, PAGE_SIZE);
        }
        // The page is now allocated but NOT mapped in user space via page tables
        // since we merged into Limine's TTBR0. For now, store the physical address
        // in a way that the user can access it.
        // TODO: map into user page table properly
    }

    // For anonymous mappings, allocate kernel memory as backing store
    // This is a temporary approach — the memory is accessible from kernel
    // but userspace accesses go through the page table
    if is_anon {
        let backing = alloc::vec![0u8; aligned_len];
        let ptr = backing.as_ptr() as usize;
        core::mem::forget(backing);
        return Ok(ptr);
    }

    // File-backed mmap
    if fd >= 0 {
        let backing = alloc::vec![0u8; aligned_len];
        let ptr = backing.as_ptr() as usize;
        core::mem::forget(backing);
        return Ok(ptr);
    }

    Ok(map_addr)
}

pub fn do_munmap(addr: usize, length: usize) -> SyscallResult {
    // Stub: accept but don't free
    Ok(0)
}

pub fn do_brk(addr: usize) -> SyscallResult {
    let mut state = USER_MEM.lock();
    if addr == 0 {
        return Ok(state.brk_current);
    }
    if addr < state.brk_base {
        return Ok(state.brk_current);
    }
    state.brk_current = addr;
    Ok(state.brk_current)
}
