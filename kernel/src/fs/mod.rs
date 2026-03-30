pub mod vfs;
mod tmpfs;
mod devfs;
mod procfs;
mod pipe;
mod fdtable;
pub mod ext4;

pub use fdtable::*;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::syscall::{SyscallResult, EBADF, ENOSYS, EINVAL, ENOENT, EEXIST, ENOTDIR, ENOMEM, EISDIR};

/// Initialize the filesystem
pub fn init() {
    vfs::init();
    devfs::init();
    procfs::init();
    tmpfs::init();
    log::info!("VFS initialized");
}

// Syscall implementations that delegate to VFS

pub fn fd_write(fd: u32, buf: u64, count: usize) -> SyscallResult {
    let file = fdtable::get_file(fd)?;
    file.lock().write(buf, count)
}

pub fn fd_read(fd: u32, buf: u64, count: usize) -> SyscallResult {
    let file = fdtable::get_file(fd)?;
    file.lock().read(buf, count)
}

pub fn openat(dirfd: i32, path: &str, flags: u32, mode: u32) -> SyscallResult {
    let node = vfs::resolve_path(path)?;
    let file = vfs::open_node(node, flags)?;
    fdtable::alloc_fd(file)
}

pub fn close(fd: u32) -> SyscallResult {
    fdtable::close_fd(fd)
}

pub fn lseek(fd: u32, offset: i64, whence: u32) -> SyscallResult {
    let file = fdtable::get_file(fd)?;
    file.lock().lseek(offset, whence)
}

pub fn fstat(fd: u32, statbuf: u64) -> SyscallResult {
    let file = fdtable::get_file(fd)?;
    file.lock().stat(statbuf)
}

pub fn fstatat(dirfd: i32, path: &str, statbuf: u64, flags: u32) -> SyscallResult {
    let node = vfs::resolve_path(path)?;
    vfs::stat_node(&node, statbuf)
}

pub fn getcwd(buf: u64, size: usize) -> SyscallResult {
    let cwd = vfs::current_dir();
    let bytes = cwd.as_bytes();
    if bytes.len() + 1 > size {
        return Err(crate::syscall::ERANGE);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, bytes.len());
        *((buf + bytes.len() as u64) as *mut u8) = 0;
    }
    Ok(buf as usize)
}

pub fn chdir(path: &str) -> SyscallResult {
    vfs::set_current_dir(path)
}

pub fn mkdirat(dirfd: i32, path: &str, mode: u32) -> SyscallResult {
    vfs::mkdir(path, mode)
}

pub fn unlinkat(dirfd: i32, path: &str, flags: u32) -> SyscallResult {
    vfs::unlink(path, flags)
}

pub fn dup(oldfd: u32) -> SyscallResult {
    fdtable::dup_fd(oldfd)
}

pub fn dup3(oldfd: u32, newfd: u32, flags: u32) -> SyscallResult {
    fdtable::dup3_fd(oldfd, newfd, flags)
}

pub fn pipe2(pipefd: u64, flags: u32) -> SyscallResult {
    let (read_file, write_file) = pipe::create_pipe()?;
    let rfd = fdtable::alloc_fd(read_file)?;
    let wfd = fdtable::alloc_fd(write_file)?;
    unsafe {
        *(pipefd as *mut i32) = rfd as i32;
        *((pipefd + 4) as *mut i32) = wfd as i32;
    }
    Ok(0)
}

pub fn fcntl(fd: u32, cmd: u32, arg: u64) -> SyscallResult {
    match cmd {
        1 => Ok(0),          // F_GETFD -> no flags
        2 => Ok(0),          // F_SETFD -> accept
        3 => Ok(0o2),        // F_GETFL -> O_RDWR
        4 => Ok(0),          // F_SETFL -> accept
        _ => Ok(0),
    }
}

pub fn do_ioctl(fd: u32, request: u64, arg: u64) -> SyscallResult {
    let file = fdtable::get_file(fd)?;
    file.lock().ioctl(request, arg)
}

pub fn epoll_create1(flags: u32) -> SyscallResult {
    // Create an epoll fd - stub implementation
    let file = Arc::new(Mutex::new(vfs::FileObject::new_epoll()));
    fdtable::alloc_fd(file)
}

pub fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: u64) -> SyscallResult {
    Ok(0) // stub
}

pub fn epoll_pwait(epfd: i32, events: u64, maxevents: i32, timeout: i32) -> SyscallResult {
    if timeout > 0 {
        // Simple: sleep for timeout ms
        let freq: u64;
        unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) freq) };
        let ticks = (timeout as u64 * freq) / 1000;
        let start: u64;
        unsafe { core::arch::asm!("mrs {}, CNTVCT_EL0", out(reg) start) };
        loop {
            let now: u64;
            unsafe { core::arch::asm!("mrs {}, CNTVCT_EL0", out(reg) now) };
            if now - start >= ticks { break; }
            core::hint::spin_loop();
        }
    }
    Ok(0) // no events ready
}

pub fn eventfd2(initval: u32, flags: u32) -> SyscallResult {
    let file = Arc::new(Mutex::new(vfs::FileObject::new_eventfd(initval)));
    fdtable::alloc_fd(file)
}

pub fn memfd_create(flags: u32) -> SyscallResult {
    let file = Arc::new(Mutex::new(vfs::FileObject::new_memfd()));
    fdtable::alloc_fd(file)
}
