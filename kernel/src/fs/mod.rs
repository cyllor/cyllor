pub mod vfs;
mod tmpfs;
mod devfs;
mod procfs;
mod pipe;
pub mod fdtable;
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
    // Special case: /dev/ptmx — allocate a new PTY pair
    if path == "/dev/ptmx" || path == "/dev/pts/ptmx" {
        let id = crate::drivers::pty::alloc_pty();
        // Unlock immediately (glibc expects unlocked after open)
        if let Some(pty_arc) = crate::drivers::pty::get_pty(id) {
            pty_arc.lock().locked = false;
        }
        let file = Arc::new(Mutex::new(vfs::FileObject {
            inode: None, offset: 0, flags,
            ftype: vfs::FileType::PtyMaster,
            special_data: Some(vfs::SpecialData::PtyMaster(id)),
        }));
        return fdtable::alloc_fd(file);
    }

    // Special case: /dev/pts/N — open PTY slave
    if path.starts_with("/dev/pts/") {
        if let Ok(id) = path[9..].parse::<u32>() {
            if crate::drivers::pty::get_pty(id).is_some() {
                let file = Arc::new(Mutex::new(vfs::FileObject {
                    inode: None, offset: 0, flags,
                    ftype: vfs::FileType::PtySlave,
                    special_data: Some(vfs::SpecialData::PtySlave(id)),
                }));
                return fdtable::alloc_fd(file);
            }
        }
        return Err(crate::syscall::ENOENT);
    }

    // Try VFS
    if let Ok(node) = vfs::resolve_path(path) {
        // Check if this is /dev/ptmx chardev (5, 2)
        {
            let n = node.lock();
            if n.itype == vfs::InodeType::CharDevice && n.dev_major == 5 && n.dev_minor == 2 {
                drop(n);
                let id = crate::drivers::pty::alloc_pty();
                if let Some(pty_arc) = crate::drivers::pty::get_pty(id) {
                    pty_arc.lock().locked = false;
                }
                let file = Arc::new(Mutex::new(vfs::FileObject {
                    inode: Some(node.clone()), offset: 0, flags,
                    ftype: vfs::FileType::PtyMaster,
                    special_data: Some(vfs::SpecialData::PtyMaster(id)),
                }));
                return fdtable::alloc_fd(file);
            }
        }
        let file = vfs::open_node(node, flags)?;
        return fdtable::alloc_fd(file);
    }

    Err(crate::syscall::ENOENT)
}

fn ensure_vfs_parents(path: &str) {
    let parts: alloc::vec::Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut current = alloc::string::String::new();
    for part in parts {
        current.push('/');
        current.push_str(part);
        if vfs::resolve_path(&current).is_err() {
            // Create directory
            if let Some(slash) = current.rfind('/') {
                let parent_path = if slash == 0 { "/" } else { &current[..slash] };
                let dirname = &current[slash+1..];
                if let Ok(parent) = vfs::resolve_path(parent_path) {
                    let mut pn = parent.lock();
                    pn.children.insert(alloc::string::String::from(dirname),
                        Arc::new(Mutex::new(vfs::Inode::new_dir(0o755))));
                }
            }
        }
    }
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
        let freq = crate::arch::counter_freq();
        let ticks = (timeout as u64 * freq) / 1000;
        let start = crate::arch::read_counter();
        loop {
            if crate::arch::read_counter() - start >= ticks { break; }
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
