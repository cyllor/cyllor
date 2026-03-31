use super::{SyscallResult, ENOMEM, ENOSYS, EINVAL};

pub fn sys_mmap(addr: u64, length: u64, prot: u32, flags: u32, fd: i32, offset: i64) -> SyscallResult {
    crate::mm::do_mmap(addr as usize, length as usize, prot, flags, fd, offset)
}

pub fn sys_munmap(addr: u64, length: u64) -> SyscallResult {
    crate::mm::do_munmap(addr as usize, length as usize)
}

pub fn sys_mprotect(addr: u64, len: u64, prot: u32) -> SyscallResult {
    // Stub: accept but don't change protections yet
    Ok(0)
}

pub fn sys_brk(addr: u64) -> SyscallResult {
    crate::mm::do_brk(addr as usize)
}

pub fn sys_getrandom(buf: u64, buflen: u64, _flags: u32) -> SyscallResult {
    let len = buflen as usize;
    let mut data = alloc::vec![0u8; len];
    let mut seed: u64 = 0;
    unsafe { core::arch::asm!("mrs {}, CNTVCT_EL0", out(reg) seed) };
    for byte in data.iter_mut() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        *byte = (seed >> 33) as u8;
    }
    let _ = super::fs::copy_to_user(buf, &data);
    Ok(len)
}

pub fn sys_mremap(old_addr: u64, old_size: u64, new_size: u64, flags: u32, new_addr: u64) -> SyscallResult {
    // Simple: just allocate new memory and return it
    // In a real OS we'd remap the pages
    if new_size <= old_size {
        return Ok(old_addr as usize);
    }
    // Allocate new region
    let new = crate::mm::do_mmap(0, new_size as usize, 3, 0x22, -1, 0)?; // PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANON
    // Copy old data
    unsafe {
        core::ptr::copy_nonoverlapping(old_addr as *const u8, new as *mut u8, old_size as usize);
    }
    Ok(new)
}

pub fn sys_memfd_create(name: u64, flags: u32) -> SyscallResult {
    // Create anonymous file backed by memory
    crate::fs::memfd_create(flags)
}
