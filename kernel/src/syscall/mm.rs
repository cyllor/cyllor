use super::{EFAULT, SyscallResult};

pub fn sys_mmap(addr: u64, length: u64, prot: u32, flags: u32, fd: i32, offset: i64) -> SyscallResult {
    crate::mm::do_mmap(addr as usize, length as usize, prot, flags, fd, offset)
}

pub fn sys_munmap(addr: u64, length: u64) -> SyscallResult {
    crate::mm::do_munmap(addr as usize, length as usize)
}

pub fn sys_mprotect(addr: u64, len: u64, prot: u32) -> SyscallResult {
    crate::mm::do_mprotect(addr as usize, len as usize, prot)
}

pub fn sys_brk(addr: u64) -> SyscallResult {
    crate::mm::do_brk(addr as usize)
}

pub fn sys_getrandom(buf: u64, buflen: u64, _flags: u32) -> SyscallResult {
    let len = buflen as usize;
    let mut data = alloc::vec![0u8; len];
    let mut seed: u64 = crate::arch::read_counter();
    for byte in data.iter_mut() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        *byte = (seed >> 33) as u8;
    }
    let _ = super::fs::copy_to_user(buf, &data);
    Ok(len)
}

pub fn sys_mremap(old_addr: u64, old_size: u64, new_size: u64, flags: u32, new_addr: u64) -> SyscallResult {
    const MREMAP_MAYMOVE: u32 = 1;
    const MREMAP_FIXED: u32 = 2;
    if old_addr == 0 || old_size == 0 || new_size == 0 {
        return Err(super::EINVAL);
    }
    if (flags & MREMAP_FIXED) != 0 && (flags & MREMAP_MAYMOVE) == 0 {
        return Err(super::EINVAL);
    }

    // In-place shrink.
    if new_size <= old_size {
        if new_size < old_size {
            let tail = old_addr + new_size;
            let _ = crate::mm::do_munmap(tail as usize, (old_size - new_size) as usize);
        }
        return Ok(old_addr as usize);
    }

    if (flags & MREMAP_MAYMOVE) == 0 {
        return Err(super::ENOMEM);
    }

    // Allocate new region (fixed if requested).
    let target = if (flags & MREMAP_FIXED) != 0 { new_addr } else { 0 };
    let mmap_flags = if target != 0 { 0x32 } else { 0x22 }; // MAP_PRIVATE|MAP_ANON|(MAP_FIXED)
    let new = crate::mm::do_mmap(target as usize, new_size as usize, 3, mmap_flags, -1, 0)?;
    // Copy old data through user copy helpers.
    let mut temp = alloc::vec![0u8; old_size as usize];
    super::fs::copy_from_user(old_addr, &mut temp).map_err(|_| EFAULT)?;
    super::fs::copy_to_user(new as u64, &temp).map_err(|_| EFAULT)?;
    let _ = crate::mm::do_munmap(old_addr as usize, old_size as usize);
    Ok(new)
}

pub fn sys_memfd_create(name: u64, flags: u32) -> SyscallResult {
    // Create anonymous file backed by memory
    crate::fs::memfd_create(flags)
}

pub fn sys_shmget(key: i32, size: u64, shmflg: i32) -> SyscallResult {
    crate::mm::do_shmget(key, size as usize, shmflg)
}

pub fn sys_shmat(shmid: i32, shmaddr: u64, shmflg: i32) -> SyscallResult {
    crate::mm::do_shmat(shmid, shmaddr, shmflg)
}

pub fn sys_shmctl(shmid: i32, cmd: i32, buf: u64) -> SyscallResult {
    crate::mm::do_shmctl(shmid, cmd, buf)
}

pub fn sys_shmdt(shmaddr: u64) -> SyscallResult {
    crate::mm::do_shmdt(shmaddr)
}
