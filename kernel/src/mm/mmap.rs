// User memory management: mmap, munmap, brk

use crate::syscall::{SyscallResult, ENOMEM, EINVAL};
use crate::mm::pmm;
use crate::arch::{AddressSpace, PageFlags};
use spin::Mutex;

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
            brk_base: crate::arch::USER_BRK_BASE,
            brk_current: crate::arch::USER_BRK_BASE,
            mmap_next: crate::arch::USER_MMAP_BASE,
        }
    }
}

pub fn init() {}

/// Set the brk base from the ELF loader (brk = end of BSS segment)
pub fn set_brk_base(addr: usize) {
    let mut state = USER_MEM.lock();
    state.brk_base = addr;
    state.brk_current = addr;
    log::debug!("brk_base set to 0x{:x}", addr);
}

/// Allocate user virtual memory and map it in the active page table
pub fn do_mmap(addr: usize, length: usize, prot: u32, flags: u32, fd: i32, offset: i64) -> SyscallResult {
    let aligned_len = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    if aligned_len == 0 {
        return Err(EINVAL);
    }

    let is_fixed = (flags & 0x10) != 0;

    let map_addr = if is_fixed && addr != 0 {
        addr
    } else {
        let mut state = USER_MEM.lock();
        let a = state.mmap_next;
        state.mmap_next += aligned_len;
        a
    };

    let hhdm = crate::arch::hhdm_offset();
    let l0_phys = crate::arch::read_user_page_table_root();

    // Determine page flags from prot
    let writable = (prot & 2) != 0; // PROT_WRITE
    let executable = (prot & 4) != 0; // PROT_EXEC
    let page_flags = PageFlags {
        readable: true,
        writable,
        executable,
        user: true,
        device: false,
    };

    // Allocate and map each page
    let num_pages = aligned_len / PAGE_SIZE;
    for i in 0..num_pages {
        let virt = (map_addr + i * PAGE_SIZE) as u64;
        let phys = pmm::alloc_page().ok_or(ENOMEM)? as u64;
        unsafe { core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, PAGE_SIZE); }
        crate::arch::map_user_page(l0_phys, virt, phys, page_flags);

        // For file-backed mappings, copy file data into the page
        if fd >= 0 {
            let file_offset = offset as usize + i * PAGE_SIZE;
            if let Ok(file) = crate::fs::fdtable::get_file(fd as u32) {
                let mut f = file.lock();
                let old_off = f.offset;
                f.offset = file_offset;
                // Read directly to physical page via HHDM
                if let Some(ref inode) = f.inode {
                    let node = inode.lock();
                    let avail = node.data.len().saturating_sub(file_offset);
                    let to_copy = PAGE_SIZE.min(avail);
                    if to_copy > 0 {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                node.data[file_offset..].as_ptr(),
                                (phys + hhdm) as *mut u8,
                                to_copy,
                            );
                        }
                    }
                }
                f.offset = old_off;
            }
        }
    }

    Ok(map_addr)
}

pub fn do_munmap(addr: usize, length: usize) -> SyscallResult {
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

    let hhdm = crate::arch::hhdm_offset();
    let l0_phys = crate::arch::read_user_page_table_root();

    // Map new pages between current brk and requested addr
    let old = (state.brk_current + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let new = (addr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    let flags = PageFlags::USER_RW;
    let mut page_addr = old;
    while page_addr < new {
        let phys = pmm::alloc_page().unwrap_or(0) as u64;
        if phys == 0 { break; }
        unsafe { core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, PAGE_SIZE); }
        crate::arch::map_user_page(l0_phys, page_addr as u64, phys, flags);
        page_addr += PAGE_SIZE;
    }

    state.brk_current = addr;
    Ok(state.brk_current)
}

