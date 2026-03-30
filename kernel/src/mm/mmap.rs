// User memory management: mmap, munmap, brk
// Maps physical pages into the user's address space via the merged TTBR0 page table

use crate::syscall::{SyscallResult, ENOMEM, EINVAL};
use crate::mm::pmm;
use crate::arch::aarch64::paging::{AddressSpace, PageFlags};
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
            brk_base: 0x0000_0060_0000_0000,     // 384 GiB
            brk_current: 0x0000_0060_0000_0000,
            mmap_next: 0x0000_0070_0000_0000,     // 448 GiB
        }
    }
}

pub fn init() {}

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

    let hhdm = crate::arch::aarch64::hhdm_offset();

    // Get the current TTBR0 (merged Limine + user page table)
    let ttbr0: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr0) };
    let l0_phys = ttbr0 & 0x0000_FFFF_FFFF_F000;

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

        // Zero the page via HHDM
        unsafe {
            core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, PAGE_SIZE);
        }

        // Map in the active page table
        map_page_in_ttbr0(l0_phys, virt, phys, page_flags, hhdm);
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

    let hhdm = crate::arch::aarch64::hhdm_offset();
    let ttbr0: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr0) };
    let l0_phys = ttbr0 & 0x0000_FFFF_FFFF_F000;

    // Map new pages between current brk and requested addr
    let old = (state.brk_current + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let new = (addr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    let flags = PageFlags::USER_RW;
    let mut page_addr = old;
    while page_addr < new {
        let phys = pmm::alloc_page().unwrap_or(0) as u64;
        if phys == 0 { break; }
        unsafe { core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, PAGE_SIZE); }
        map_page_in_ttbr0(l0_phys, page_addr as u64, phys, flags, hhdm);
        page_addr += PAGE_SIZE;
    }

    state.brk_current = addr;
    Ok(state.brk_current)
}

/// Map a single page in the active 4-level page table (TTBR0)
fn map_page_in_ttbr0(l0_phys: u64, virt: u64, phys: u64, flags: PageFlags, hhdm: u64) {
    let indices = [
        ((virt >> 39) & 0x1FF) as usize,
        ((virt >> 30) & 0x1FF) as usize,
        ((virt >> 21) & 0x1FF) as usize,
        ((virt >> 12) & 0x1FF) as usize,
    ];

    let pte_flags = flags.to_pte();
    let mut table_phys = l0_phys;

    // Walk/create L0 → L1 → L2
    for level in 0..3 {
        let table_virt = (table_phys + hhdm) as *mut u64;
        let entry = unsafe { core::ptr::read_volatile(table_virt.add(indices[level])) };

        if entry & 1 == 0 {
            // Allocate new table
            let new_table = pmm::alloc_page().unwrap() as u64;
            unsafe { core::ptr::write_bytes((new_table + hhdm) as *mut u8, 0, PAGE_SIZE); }
            let new_entry = new_table | 0x3; // Valid + Table
            unsafe { core::ptr::write_volatile(table_virt.add(indices[level]), new_entry); }
            table_phys = new_table;
        } else {
            table_phys = entry & 0x0000_FFFF_FFFF_F000;
        }
    }

    // Write L3 entry
    let l3_virt = (table_phys + hhdm) as *mut u64;
    let l3_entry = (phys & 0x0000_FFFF_FFFF_F000) | pte_flags | 0x3 | (1 << 10); // Valid + Page + AF
    unsafe { core::ptr::write_volatile(l3_virt.add(indices[3]), l3_entry); }
}
