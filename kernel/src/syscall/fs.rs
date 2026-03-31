use super::{SyscallResult, EBADF, ENOSYS, EINVAL};

pub fn sys_write(fd: u64, buf: u64, count: u64) -> SyscallResult {
    match fd as u32 {
        1 | 2 => {
            // stdout / stderr -> UART
            // buf is a user virtual address — translate to physical via HHDM
            // Read TTBR0_EL1 to get the current user page table
            let ttbr0: u64;
            unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr0) };
            let hhdm = crate::arch::aarch64::hhdm_offset();

            // Walk page table manually to translate user VA to physical
            match walk_user_page_table(ttbr0, buf, hhdm) {
                Some(pa) => {
                    let kva = pa + hhdm;
                    let slice = unsafe { core::slice::from_raw_parts(kva as *const u8, count as usize) };
                    for &b in slice {
                        crate::drivers::uart::write_byte(b);
                    }
                    Ok(count as usize)
                }
                None => {
                    crate::drivers::uart::early_print("[write: VA translation failed]\n");
                    Err(super::EFAULT)
                }
            }
        }
        _ => {
            crate::fs::fd_write(fd as u32, buf, count as usize)
        }
    }
}

pub fn sys_read(fd: u64, buf: u64, count: u64) -> SyscallResult {
    crate::fs::fd_read(fd as u32, buf, count as usize)
}

pub fn sys_openat(dirfd: i32, pathname: u64, flags: u32, mode: u32) -> SyscallResult {
    let mut path_buf = [0u8; 256];
    let len = read_user_string(pathname, &mut path_buf)?;
    let path = core::str::from_utf8(&path_buf[..len]).unwrap_or("");
    // Fast path: try VFS, fall back to ENOENT
    let result = crate::fs::openat(dirfd, path, flags, mode);
    result
}

/// Read a null-terminated string from user memory, demand-paging as needed
fn read_user_string(addr: u64, buf: &mut [u8]) -> Result<usize, i32> {
    let ttbr0: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr0) };
    let hhdm = crate::arch::aarch64::hhdm_offset();
    let l0_phys = ttbr0 & 0x0000_FFFF_FFFF_F000;

    let mut len = 0;
    let mut uaddr = addr;
    while len < buf.len() - 1 {
        let pa = match walk_user_page_table(ttbr0, uaddr, hhdm) {
            Some(pa) => pa,
            None => {
                // Demand-page: allocate and map a zero page
                let page = uaddr & !0xFFF;
                let phys = crate::mm::pmm::alloc_page().ok_or(super::ENOMEM)? as u64;
                unsafe { core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, 4096); }
                crate::mm::mmap::map_page_in_ttbr0(
                    l0_phys, page, phys,
                    crate::arch::aarch64::paging::PageFlags::USER_RW, hhdm,
                );
                match walk_user_page_table(ttbr0, uaddr, hhdm) {
                    Some(pa) => pa,
                    None => return Err(super::EFAULT),
                }
            }
        };
        let kva = pa + hhdm;
        let b = unsafe { *(kva as *const u8) };
        if b == 0 { break; }
        buf[len] = b;
        len += 1;
        uaddr += 1;
    }
    Ok(len)
}

pub fn sys_close(fd: u32) -> SyscallResult {
    crate::fs::close(fd)
}

pub fn sys_lseek(fd: u32, offset: i64, whence: u32) -> SyscallResult {
    crate::fs::lseek(fd, offset, whence)
}

pub fn sys_fstat(fd: u32, statbuf: u64) -> SyscallResult {
    crate::fs::fstat(fd, statbuf)
}

pub fn sys_newfstatat(dirfd: i32, pathname: u64, statbuf: u64, flags: u32) -> SyscallResult {
    let mut path_buf = [0u8; 256];
    let len = read_user_string(pathname, &mut path_buf)?;
    let path = core::str::from_utf8(&path_buf[..len]).unwrap_or("");
    crate::fs::fstatat(dirfd, path, statbuf, flags)
}

pub fn sys_getcwd(buf: u64, size: u64) -> SyscallResult {
    crate::fs::getcwd(buf, size as usize)
}

pub fn sys_chdir(path: u64) -> SyscallResult {
    let p = unsafe { cstr_from_user(path)? };
    crate::fs::chdir(p)
}

pub fn sys_mkdirat(dirfd: i32, pathname: u64, mode: u32) -> SyscallResult {
    let path = unsafe { cstr_from_user(pathname)? };
    crate::fs::mkdirat(dirfd, path, mode)
}

pub fn sys_unlinkat(dirfd: i32, pathname: u64, flags: u32) -> SyscallResult {
    let path = unsafe { cstr_from_user(pathname)? };
    crate::fs::unlinkat(dirfd, path, flags)
}

pub fn sys_writev(fd: u32, iov: u64, iovcnt: u32) -> SyscallResult {
    let mut total = 0usize;
    for i in 0..iovcnt {
        let iovec_ptr = iov + (i as u64) * 16;
        let base = unsafe { *(iovec_ptr as *const u64) };
        let len = unsafe { *((iovec_ptr + 8) as *const u64) };
        if len > 0 {
            total += sys_write(fd as u64, base, len)?;
        }
    }
    Ok(total)
}

pub fn sys_readv(fd: u32, iov: u64, iovcnt: u32) -> SyscallResult {
    let mut total = 0usize;
    for i in 0..iovcnt {
        let iovec_ptr = iov + (i as u64) * 16;
        let base = unsafe { *(iovec_ptr as *const u64) };
        let len = unsafe { *((iovec_ptr + 8) as *const u64) };
        if len > 0 {
            total += sys_read(fd as u64, base, len)?;
        }
    }
    Ok(total)
}

pub fn sys_dup(oldfd: u32) -> SyscallResult {
    crate::fs::dup(oldfd)
}

pub fn sys_dup3(oldfd: u32, newfd: u32, flags: u32) -> SyscallResult {
    crate::fs::dup3(oldfd, newfd, flags)
}

pub fn sys_pipe2(pipefd: u64, flags: u32) -> SyscallResult {
    crate::fs::pipe2(pipefd, flags)
}

pub fn sys_fcntl(fd: u32, cmd: u32, arg: u64) -> SyscallResult {
    crate::fs::fcntl(fd, cmd, arg)
}

/// Write kernel data to a user virtual address via page table translation
pub fn copy_to_user(user_addr: u64, data: &[u8]) -> Result<(), i32> {
    let ttbr0: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr0) };
    let hhdm = crate::arch::aarch64::hhdm_offset();

    let mut offset = 0;
    while offset < data.len() {
        let uaddr = user_addr + offset as u64;
        match walk_user_page_table(ttbr0, uaddr, hhdm) {
            Some(pa) => {
                let kva = pa + hhdm;
                let page_rem = 4096 - (uaddr & 0xFFF) as usize;
                let chunk = page_rem.min(data.len() - offset);
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        data[offset..].as_ptr(),
                        kva as *mut u8,
                        chunk,
                    );
                }
                offset += chunk;
            }
            None => {
                // Page fault — try to demand-page it
                let page_addr = uaddr & !0xFFF;
                let phys = crate::mm::pmm::alloc_page().ok_or(super::ENOMEM)? as u64;
                unsafe { core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, 4096); }
                let l0_phys = ttbr0 & 0x0000_FFFF_FFFF_F000;
                crate::mm::mmap::map_page_in_ttbr0(
                    l0_phys, page_addr, phys,
                    crate::arch::aarch64::paging::PageFlags::USER_RW, hhdm,
                );
                // Retry this offset
            }
        }
    }
    Ok(())
}

/// Read from user virtual address to kernel buffer
pub fn copy_from_user(user_addr: u64, buf: &mut [u8]) -> Result<(), i32> {
    let ttbr0: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr0) };
    let hhdm = crate::arch::aarch64::hhdm_offset();

    let mut offset = 0;
    while offset < buf.len() {
        let uaddr = user_addr + offset as u64;
        match walk_user_page_table(ttbr0, uaddr, hhdm) {
            Some(pa) => {
                let kva = pa + hhdm;
                let page_rem = 4096 - (uaddr & 0xFFF) as usize;
                let chunk = page_rem.min(buf.len() - offset);
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        kva as *const u8,
                        buf[offset..].as_mut_ptr(),
                        chunk,
                    );
                }
                offset += chunk;
            }
            None => return Err(super::EFAULT),
        }
    }
    Ok(())
}

pub fn print_hex(val: u64) {
    for i in (0..16).rev() {
        let nibble = ((val >> (i * 4)) & 0xF) as u8;
        let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
        crate::drivers::uart::write_byte(c);
    }
}

/// Walk user page table (TTBR0) to translate VA to PA
pub fn walk_user_page_table(ttbr0: u64, va: u64, hhdm: u64) -> Option<u64> {
    let indices = [
        ((va >> 39) & 0x1FF) as usize,
        ((va >> 30) & 0x1FF) as usize,
        ((va >> 21) & 0x1FF) as usize,
        ((va >> 12) & 0x1FF) as usize,
    ];

    let mut table_phys = ttbr0 & 0x0000_FFFF_FFFF_F000;

    for level in 0..3 {
        let table_virt = (table_phys + hhdm) as *const u64;
        let entry = unsafe { core::ptr::read_volatile(table_virt.add(indices[level])) };
        if entry & 1 == 0 {
            return None;
        }
        table_phys = entry & 0x0000_FFFF_FFFF_F000;
    }

    let l3_virt = (table_phys + hhdm) as *const u64;
    let entry = unsafe { core::ptr::read_volatile(l3_virt.add(indices[3])) };
    if entry & 1 == 0 {
        return None;
    }

    let page_phys = entry & 0x0000_FFFF_FFFF_F000;
    Some(page_phys | (va & 0xFFF))
}

pub fn sys_getdents64(fd: u32, dirp: u64, count: u32) -> SyscallResult {
    // Return 0 = end of directory
    Ok(0)
}

pub fn sys_readlinkat(dirfd: i32, pathname: u64, buf: u64, bufsiz: u32) -> SyscallResult {
    let mut path_buf = [0u8; 256];
    let len = read_user_string(pathname, &mut path_buf)?;
    let path = core::str::from_utf8(&path_buf[..len]).unwrap_or("");
    // /proc/self/exe -> return the running program name
    if path == "/proc/self/exe" {
        let exe = b"/bin/hello";
        let len = (exe.len()).min(bufsiz as usize);
        unsafe {
            core::ptr::copy_nonoverlapping(exe.as_ptr(), buf as *mut u8, len);
        }
        return Ok(len);
    }
    Err(super::EINVAL)
}

unsafe fn cstr_from_user(ptr: u64) -> Result<&'static str, i32> {
    if ptr == 0 {
        return Err(EINVAL);
    }
    let mut len = 0;
    loop {
        if unsafe { *((ptr + len) as *const u8) } == 0 {
            break;
        }
        len += 1;
        if len > 4096 {
            return Err(EINVAL);
        }
    }
    let slice = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    core::str::from_utf8(slice).map_err(|_| EINVAL)
}
