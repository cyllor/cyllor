// User memory management: mmap, munmap, mprotect, brk

use alloc::string::ToString;
use alloc::vec::Vec;
use crate::arch::PageAttr;
use crate::mm::pmm;
use crate::mm::vmm::{MAP_ANONYMOUS, MAP_PRIVATE, MAP_SHARED, PROT_EXEC, PROT_READ, PROT_WRITE};
use crate::sched::process::PROCESS_TABLE;
use crate::syscall::{EINVAL, ENOMEM, ESRCH, SyscallResult};

const PAGE_SIZE: usize = 4096;
const VALID_PROT_MASK: u32 = PROT_READ | PROT_WRITE | PROT_EXEC;

#[inline]
fn align_up(v: usize) -> usize {
    (v + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

fn prot_to_page_attr(prot: u32) -> PageAttr {
    let readable = (prot & PROT_READ) != 0;
    let writable = (prot & PROT_WRITE) != 0;
    let executable = (prot & PROT_EXEC) != 0;
    PageAttr {
        readable: readable || writable || executable,
        writable,
        executable,
        user: true,
        device: false,
    }
}

fn current_pid() -> u64 {
    crate::sched::process::current_pid()
}

fn update_heap_vma_locked(proc: &mut crate::sched::process::Process, old_heap_end: usize) {
    let mut vmm = proc.vmm.lock();
    if old_heap_end > proc.brk_base {
        vmm.unmap(proc.brk_base as u64, (old_heap_end - proc.brk_base) as u64);
    }
    let new_heap_end = align_up(proc.brk_current);
    if new_heap_end > proc.brk_base {
        vmm.map(
            proc.brk_base as u64,
            new_heap_end as u64,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            0,
            Some("[heap]".to_string()),
        );
    }
}

pub fn init() {}

/// Legacy hook kept for loader call sites; now updates the current process state.
pub fn set_brk_base(addr: usize) {
    let pid = current_pid();
    let mut table = PROCESS_TABLE.lock();
    if let Some(proc) = table.get_mut(&pid) {
        let old_heap_end = align_up(proc.brk_current);
        proc.brk_base = addr;
        proc.brk_current = addr;
        update_heap_vma_locked(proc, old_heap_end);
    }
}

/// Reserve userspace VA range for SysV SHM attachment.
pub fn shm_alloc_addr(size: usize) -> usize {
    let pid = current_pid();
    let mut table = PROCESS_TABLE.lock();
    if let Some(proc) = table.get_mut(&pid) {
        let aligned = align_up(size.max(PAGE_SIZE));
        let addr = align_up(proc.mmap_next);
        proc.mmap_next = addr.saturating_add(aligned);
        return addr;
    }
    crate::arch::USER_MMAP_BASE
}

/// Allocate user virtual memory and map it in the active page table.
pub fn do_mmap(addr: usize, length: usize, prot: u32, flags: u32, fd: i32, offset: i64) -> SyscallResult {
    let aligned_len = align_up(length);
    if aligned_len == 0 {
        return Err(EINVAL);
    }
    if (prot & !VALID_PROT_MASK) != 0 {
        return Err(EINVAL);
    }
    let is_private = (flags & MAP_PRIVATE) != 0;
    let is_shared = (flags & MAP_SHARED) != 0;
    if is_private == is_shared {
        return Err(EINVAL);
    }
    let is_anon = (flags & MAP_ANONYMOUS) != 0;
    if !is_anon && fd < 0 {
        return Err(EINVAL);
    }
    if fd >= 0 && (offset < 0 || (offset as usize) % PAGE_SIZE != 0) {
        return Err(EINVAL);
    }

    let is_fixed = (flags & 0x10) != 0;
    let pid = current_pid();
    let map_addr = {
        let mut table = PROCESS_TABLE.lock();
        let proc = table.get_mut(&pid).ok_or(ESRCH)?;
        if is_fixed {
            if addr == 0 || addr % PAGE_SIZE != 0 {
                return Err(EINVAL);
            }
            addr
        } else {
            let base = align_up(proc.mmap_next);
            proc.mmap_next = base.saturating_add(aligned_len);
            base
        }
    };

    if is_fixed {
        let _ = do_munmap(map_addr, aligned_len);
    }

    let hhdm = crate::arch::hhdm_offset();
    let root_phys = crate::arch::read_user_page_table_root();
    if root_phys == 0 {
        return Err(ENOMEM);
    }

    let page_flags = prot_to_page_attr(prot);
    let num_pages = aligned_len / PAGE_SIZE;
    let mut mapped: Vec<(u64, u64)> = Vec::with_capacity(num_pages);

    let file = if fd >= 0 {
        crate::fs::fdtable::get_file(fd as u32).ok()
    } else {
        None
    };

    for i in 0..num_pages {
        let virt = (map_addr + i * PAGE_SIZE) as u64;
        let phys = match pmm::alloc_page() {
            Some(p) => p as u64,
            None => {
                for (v, fallback_phys) in mapped.drain(..) {
                    if let Some(p) = crate::arch::unmap_user_page(root_phys, v) {
                        pmm::free_page(p as usize);
                    } else {
                        pmm::free_page(fallback_phys as usize);
                    }
                }
                return Err(ENOMEM);
            }
        };

        unsafe { core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, PAGE_SIZE); }
        crate::arch::map_user_page(root_phys, virt, phys, page_flags);
        mapped.push((virt, phys));

        if let Some(ref fref) = file {
            let file_offset = offset as usize + i * PAGE_SIZE;
            let mut f = fref.lock();
            let old_off = f.offset;
            f.offset = file_offset;
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

    {
        let mut table = PROCESS_TABLE.lock();
        let proc = table.get_mut(&pid).ok_or(ESRCH)?;
        let name = if (flags & MAP_ANONYMOUS) != 0 {
            Some("[anon]".to_string())
        } else {
            Some(alloc::format!("[fd:{fd}]"))
        };
        proc.vmm.lock().map(
            map_addr as u64,
            (map_addr + aligned_len) as u64,
            prot,
            (if is_shared { MAP_SHARED } else { MAP_PRIVATE }) | (flags & MAP_ANONYMOUS),
            offset.max(0) as u64,
            name,
        );
    }

    Ok(map_addr)
}

pub fn do_munmap(addr: usize, length: usize) -> SyscallResult {
    if length == 0 || addr % PAGE_SIZE != 0 {
        return Err(EINVAL);
    }
    let aligned_len = align_up(length);
    let root_phys = crate::arch::read_user_page_table_root();
    if root_phys == 0 {
        return Err(ENOMEM);
    }

    let pages = aligned_len / PAGE_SIZE;
    let pid = current_pid();
    let shared_shm_pages: Vec<bool> = {
        let table = PROCESS_TABLE.lock();
        if let Some(proc) = table.get(&pid) {
            let vmm = proc.vmm.lock();
            (0..pages)
                .map(|i| {
                    let virt = (addr + i * PAGE_SIZE) as u64;
                    vmm.find(virt).map_or(false, |vma| {
                        (vma.flags & MAP_SHARED) != 0
                            && vma.name.as_deref().map_or(false, |n| n.starts_with("[shm:"))
                    })
                })
                .collect()
        } else {
            alloc::vec![false; pages]
        }
    };

    for i in 0..pages {
        let virt = (addr + i * PAGE_SIZE) as u64;
        if let Some(phys) = crate::arch::unmap_user_page(root_phys, virt) {
            if !shared_shm_pages[i] {
                pmm::free_page(phys as usize);
            }
        }
    }

    if let Some(proc) = PROCESS_TABLE.lock().get_mut(&pid) {
        proc.vmm.lock().unmap(addr as u64, aligned_len as u64);
    }
    crate::mm::shm::detach_mappings_in_range(pid, addr, aligned_len);

    Ok(0)
}

pub fn do_mprotect(addr: usize, length: usize, prot: u32) -> SyscallResult {
    if length == 0 || addr % PAGE_SIZE != 0 {
        return Err(EINVAL);
    }
    if (prot & !VALID_PROT_MASK) != 0 {
        return Err(EINVAL);
    }
    let aligned_len = align_up(length);
    let root_phys = crate::arch::read_user_page_table_root();
    if root_phys == 0 {
        return Err(ENOMEM);
    }

    let flags = prot_to_page_attr(prot);
    let pages = aligned_len / PAGE_SIZE;
    let mut page_phys: Vec<(u64, u64)> = Vec::with_capacity(pages);

    for i in 0..pages {
        let virt = (addr + i * PAGE_SIZE) as u64;
        let phys = crate::arch::translate_user_va(root_phys, virt).ok_or(EINVAL)? & !0xFFF;
        page_phys.push((virt, phys));
    }

    for (virt, phys) in page_phys {
        crate::arch::map_user_page(root_phys, virt, phys, flags);
        crate::arch::flush_user_tlb_va(virt);
    }

    let pid = current_pid();
    if let Some(proc) = PROCESS_TABLE.lock().get_mut(&pid) {
        proc.vmm.lock().mprotect(addr as u64, aligned_len as u64, prot);
    }
    Ok(0)
}

pub fn do_brk(addr: usize) -> SyscallResult {
    let pid = current_pid();
    let mut table = PROCESS_TABLE.lock();
    let proc = table.get_mut(&pid).ok_or(ESRCH)?;

    if addr == 0 {
        return Ok(proc.brk_current);
    }
    if addr < proc.brk_base {
        return Ok(proc.brk_current);
    }
    if addr == proc.brk_current {
        return Ok(proc.brk_current);
    }

    let root_phys = crate::arch::read_user_page_table_root();
    if root_phys == 0 {
        return Ok(proc.brk_current);
    }
    let hhdm = crate::arch::hhdm_offset();

    let old_brk = proc.brk_current;
    let old_aligned = align_up(old_brk);
    let new_aligned = align_up(addr);

    if new_aligned > old_aligned {
        let mut just_mapped: Vec<(u64, u64)> = Vec::new();
        let mut page_addr = old_aligned;
        while page_addr < new_aligned {
            let phys = match pmm::alloc_page() {
                Some(p) => p as u64,
                None => {
                    for (v, p) in just_mapped.drain(..) {
                        if let Some(up) = crate::arch::unmap_user_page(root_phys, v) {
                            pmm::free_page(up as usize);
                        } else {
                            pmm::free_page(p as usize);
                        }
                    }
                    return Ok(proc.brk_current);
                }
            };
            unsafe { core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, PAGE_SIZE); }
            crate::arch::map_user_page(root_phys, page_addr as u64, phys, PageAttr::USER_RW);
            just_mapped.push((page_addr as u64, phys));
            page_addr += PAGE_SIZE;
        }
    } else if new_aligned < old_aligned {
        let mut page_addr = new_aligned;
        while page_addr < old_aligned {
            if let Some(phys) = crate::arch::unmap_user_page(root_phys, page_addr as u64) {
                pmm::free_page(phys as usize);
            }
            page_addr += PAGE_SIZE;
        }
    }

    let old_heap_end = old_aligned;
    proc.brk_current = addr;
    update_heap_vma_locked(proc, old_heap_end);
    Ok(proc.brk_current)
}
