use super::{SyscallResult, EBADF, ENOSYS, EINVAL};

pub fn sys_write(fd: u64, buf: u64, count: u64) -> SyscallResult {
    match fd as u32 {
        1 | 2 => {
            // stdout / stderr -> UART
            let slice = unsafe { core::slice::from_raw_parts(buf as *const u8, count as usize) };
            for &b in slice {
                crate::drivers::uart::write_byte(b);
            }
            Ok(count as usize)
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
    let path = unsafe { cstr_from_user(pathname)? };
    crate::fs::openat(dirfd, path, flags, mode)
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
    let path = unsafe { cstr_from_user(pathname)? };
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
