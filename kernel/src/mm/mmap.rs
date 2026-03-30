use crate::syscall::{SyscallResult, ENOMEM, EINVAL};
use spin::Mutex;
use alloc::vec::Vec;

const PAGE_SIZE: usize = 4096;

// Simple bump allocator for user virtual memory
// In a real OS this would manage page tables
static USER_MEM: Mutex<UserMemState> = Mutex::new(UserMemState::new());

struct UserMemState {
    // User heap (brk)
    brk_base: usize,
    brk_current: usize,
    // mmap region
    mmap_base: usize,
    mmap_current: usize,
    // Allocated regions for tracking
    regions: Vec<MmapRegion>,
}

struct MmapRegion {
    addr: usize,
    len: usize,
}

impl UserMemState {
    const fn new() -> Self {
        Self {
            brk_base: 0x0000_0040_0000_0000, // 256 GiB
            brk_current: 0x0000_0040_0000_0000,
            mmap_base: 0x0000_0070_0000_0000,
            mmap_current: 0x0000_0070_0000_0000,
            regions: Vec::new(),
        }
    }
}

pub fn init() {
    // Nothing to initialize yet - will set up user page tables later
}

pub fn do_mmap(addr: usize, length: usize, prot: u32, flags: u32, fd: i32, offset: i64) -> SyscallResult {
    let mut state = USER_MEM.lock();
    let aligned_len = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    let map_addr = if addr != 0 && (flags & 0x10) != 0 {
        // MAP_FIXED
        addr
    } else {
        let a = state.mmap_current;
        state.mmap_current += aligned_len;
        a
    };

    state.regions.push(MmapRegion { addr: map_addr, len: aligned_len });

    // Allocate physical pages and map them
    // For now, we use the kernel allocator for anonymous mappings
    let is_anon = (flags & 0x20) != 0; // MAP_ANONYMOUS
    if is_anon {
        // Allocate backing memory
        let backing = alloc::vec![0u8; aligned_len];
        let ptr = backing.as_ptr() as usize;
        core::mem::forget(backing); // Leak intentionally - freed on munmap

        // In a real OS we'd set up page table entries
        // For now, return the kernel address directly (works until we have real userspace)
        return Ok(ptr);
    }

    // File-backed mmap
    if fd >= 0 {
        // Read file contents into mapped memory
        let backing = alloc::vec![0u8; aligned_len];
        let ptr = backing.as_ptr() as usize;
        core::mem::forget(backing);
        // TODO: actually read from fd
        return Ok(ptr);
    }

    Ok(map_addr)
}

pub fn do_munmap(addr: usize, length: usize) -> SyscallResult {
    // Stub: accept but don't actually free
    // In a real OS we'd update page tables and free physical pages
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

    // Extend brk
    state.brk_current = addr;
    Ok(state.brk_current)
}
