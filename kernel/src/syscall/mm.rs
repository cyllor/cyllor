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
    // Simple PRNG for now - not cryptographically secure
    let slice = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, buflen as usize) };
    let mut seed: u64 = 0;
    unsafe { core::arch::asm!("mrs {}, CNTVCT_EL0", out(reg) seed) };
    for byte in slice.iter_mut() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        *byte = (seed >> 33) as u8;
    }
    Ok(buflen as usize)
}

pub fn sys_memfd_create(name: u64, flags: u32) -> SyscallResult {
    // Create anonymous file backed by memory
    crate::fs::memfd_create(flags)
}
