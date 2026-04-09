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
use alloc::collections::BTreeMap;
use spin::Mutex;
use crate::syscall::{SyscallResult, EBADF, ENOSYS, EINVAL, ENOENT, EEXIST, ENOTDIR, ENOMEM, EISDIR};

#[derive(Clone, Copy)]
struct EpollWatch {
    events: u32,
    data: u64,
}

static EPOLL_TABLE: Mutex<BTreeMap<i32, BTreeMap<i32, EpollWatch>>> = Mutex::new(BTreeMap::new());

const EPOLL_CLOEXEC: u32 = 0x80000;
const EPOLL_CTL_ADD: i32 = 1;
const EPOLL_CTL_DEL: i32 = 2;
const EPOLL_CTL_MOD: i32 = 3;
const EPOLLIN: u32 = 0x001;
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;

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

pub fn alloc_pidfd(pid: u64) -> SyscallResult {
    let file = Arc::new(Mutex::new(vfs::FileObject::new_pidfd(pid)));
    fdtable::alloc_fd(file)
}

pub fn openat(dirfd: i32, path: &str, flags: u32, mode: u32) -> SyscallResult {
    let full_path = resolve_at_path(dirfd, path)?;
    // Special case: /dev/ptmx — allocate a new PTY pair
    if full_path == "/dev/ptmx" || full_path == "/dev/pts/ptmx" {
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
    if full_path.starts_with("/dev/pts/") {
        if let Ok(id) = full_path[9..].parse::<u32>() {
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
    if let Ok(node) = vfs::resolve_path(&full_path) {
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

    // Lazy-load from ext4 into VFS on first open to reduce static preload list.
    if let Ok(data) = ext4::read_file(&full_path) {
        let _ = materialize_ext4_file_to_vfs(&full_path, data);
        if let Ok(node) = vfs::resolve_path(&full_path) {
            let file = vfs::open_node(node, flags)?;
            return fdtable::alloc_fd(file);
        }
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
    if let Ok(file) = fdtable::get_file(fd) {
        if file.lock().ftype == vfs::FileType::Epoll {
            let mut ep = EPOLL_TABLE.lock();
            ep.remove(&(fd as i32));
            for watches in ep.values_mut() {
                watches.remove(&(fd as i32));
            }
        }
    }
    fdtable::close_fd(fd)
}

fn materialize_ext4_file_to_vfs(path: &str, data: alloc::vec::Vec<u8>) -> Result<(), i32> {
    if !path.starts_with('/') || path == "/" {
        return Err(EINVAL);
    }
    ensure_vfs_parents(path);
    let (parent_path, name) = path.rsplit_once('/').ok_or(EINVAL)?;
    let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
    let parent = vfs::resolve_path(parent_path)?;
    let mut pn = parent.lock();
    let mut inode = vfs::Inode::new_file(0o755);
    inode.size = data.len();
    inode.data = data;
    pn.children
        .insert(alloc::string::String::from(name), Arc::new(Mutex::new(inode)));
    Ok(())
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
    const AT_FDCWD: i32 = -100;
    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    const AT_EMPTY_PATH: u32 = 0x1000;
    const AT_NO_AUTOMOUNT: u32 = 0x800;
    const ALLOWED_FLAGS: u32 = AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH | AT_NO_AUTOMOUNT;
    if (flags & !ALLOWED_FLAGS) != 0 {
        return Err(EINVAL);
    }

    let node = if path.is_empty() {
        if (flags & AT_EMPTY_PATH) == 0 {
            return Err(EINVAL);
        }
        if dirfd == AT_FDCWD {
            vfs::resolve_path(&vfs::current_dir())?
        } else {
            if dirfd < 0 {
                return Err(EBADF);
            }
            let f = fdtable::get_file(dirfd as u32)?;
            f.lock().inode.clone().ok_or(EINVAL)?
        }
    } else {
        let full_path = resolve_at_path(dirfd, path)?;
        if (flags & AT_SYMLINK_NOFOLLOW) != 0 {
            vfs::resolve_path_lstat(&full_path)?
        } else {
            vfs::resolve_path(&full_path)?
        }
    };
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
    let full_path = resolve_at_path(dirfd, path)?;
    vfs::mkdir(&full_path, mode)
}

pub fn unlinkat(dirfd: i32, path: &str, flags: u32) -> SyscallResult {
    let full_path = resolve_at_path(dirfd, path)?;
    vfs::unlink(&full_path, flags)
}

pub fn renameat2(olddirfd: i32, oldpath: &str, newdirfd: i32, newpath: &str, flags: u32) -> SyscallResult {
    let old_full = resolve_at_path(olddirfd, oldpath)?;
    let new_full = resolve_at_path(newdirfd, newpath)?;
    vfs::rename(&old_full, &new_full, flags)
}

fn resolve_at_path(dirfd: i32, path: &str) -> Result<String, i32> {
    const AT_FDCWD: i32 = -100;
    if path.starts_with('/') {
        return Ok(String::from(path));
    }
    if dirfd == AT_FDCWD {
        let cwd = vfs::current_dir();
        if cwd == "/" {
            return Ok(alloc::format!("/{}", path));
        }
        return Ok(alloc::format!("{}/{}", cwd.trim_end_matches('/'), path));
    }
    if dirfd < 0 {
        return Err(EBADF);
    }
    let file = fdtable::get_file(dirfd as u32)?;
    let inode = file.lock().inode.clone().ok_or(EBADF)?;
    if inode.lock().itype != vfs::InodeType::Directory {
        return Err(ENOTDIR);
    }
    let base = vfs::path_of_inode(&inode).ok_or(ENOENT)?;
    if base == "/" {
        Ok(alloc::format!("/{}", path))
    } else {
        Ok(alloc::format!("{}/{}", base.trim_end_matches('/'), path))
    }
}

pub fn faccessat(_dirfd: i32, path: &str, _mode: u32, _flags: u32) -> SyscallResult {
    const AT_FDCWD: i32 = -100;
    const F_OK: u32 = 0;
    const X_OK: u32 = 1;
    const W_OK: u32 = 2;
    const R_OK: u32 = 4;
    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    const AT_EACCESS: u32 = 0x200;
    const AT_EMPTY_PATH: u32 = 0x1000;
    const ALLOWED_FLAGS: u32 = AT_SYMLINK_NOFOLLOW | AT_EACCESS | AT_EMPTY_PATH;

    if (_flags & !ALLOWED_FLAGS) != 0 {
        return Err(EINVAL);
    }
    if (_mode & !(R_OK | W_OK | X_OK)) != 0 {
        return Err(EINVAL);
    }

    let node = if path.is_empty() {
        if (_flags & AT_EMPTY_PATH) == 0 {
            return Err(EINVAL);
        }
        if _dirfd == AT_FDCWD {
            vfs::resolve_path(&vfs::current_dir())?
        } else {
            if _dirfd < 0 {
                return Err(crate::syscall::EBADF);
            }
            let f = fdtable::get_file(_dirfd as u32)?;
            f.lock().inode.clone().ok_or(EINVAL)?
        }
    } else {
        let full_path = resolve_at_path(_dirfd, path)?;
        if (_flags & AT_SYMLINK_NOFOLLOW) != 0 {
            vfs::resolve_path_lstat(&full_path)?
        } else {
            vfs::resolve_path(&full_path)?
        }
    };
    if _mode == F_OK {
        return Ok(0);
    }

    let (uid, _gid, euid, _egid) = crate::syscall::current_creds();
    let _chosen_uid = if (_flags & AT_EACCESS) != 0 { euid } else { uid };
    let _use_effective_ids = (_flags & AT_EACCESS) != 0;
    let is_root = _chosen_uid == 0;

    let n = node.lock();
    let mode = n.mode;
    let can_r = (mode & 0o444) != 0;
    let can_w = (mode & 0o222) != 0;
    let can_x = (mode & 0o111) != 0;
    if is_root {
        if (_mode & X_OK) != 0 && !can_x {
            return Err(crate::syscall::EACCES);
        }
        return Ok(0);
    }
    if ((_mode & R_OK) != 0 && !can_r) || ((_mode & W_OK) != 0 && !can_w) || ((_mode & X_OK) != 0 && !can_x) {
        return Err(crate::syscall::EACCES);
    }
    Ok(0)
}

pub fn statx(_dirfd: i32, path: &str, flags: u32, _mask: u32, statxbuf: u64) -> SyscallResult {
    const AT_FDCWD: i32 = -100;
    const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
    const AT_EMPTY_PATH: u32 = 0x1000;
    const AT_STATX_SYNC_AS_STAT: u32 = 0x0000;
    const AT_STATX_FORCE_SYNC: u32 = 0x2000;
    const AT_STATX_DONT_SYNC: u32 = 0x4000;
    const ALLOWED_FLAGS: u32 =
        AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH | AT_STATX_SYNC_AS_STAT | AT_STATX_FORCE_SYNC | AT_STATX_DONT_SYNC;
    if (flags & !ALLOWED_FLAGS) != 0 {
        return Err(EINVAL);
    }

    const STATX_TYPE: u32 = 0x0001;
    const STATX_MODE: u32 = 0x0002;
    const STATX_NLINK: u32 = 0x0004;
    const STATX_UID: u32 = 0x0008;
    const STATX_GID: u32 = 0x0010;
    const STATX_ATIME: u32 = 0x0020;
    const STATX_MTIME: u32 = 0x0040;
    const STATX_CTIME: u32 = 0x0080;
    const STATX_INO: u32 = 0x0100;
    const STATX_SIZE: u32 = 0x0200;
    const STATX_BLOCKS: u32 = 0x0400;
    const STATX_BASIC_STATS: u32 =
        STATX_TYPE | STATX_MODE | STATX_NLINK | STATX_UID | STATX_GID | STATX_ATIME | STATX_MTIME | STATX_CTIME
            | STATX_INO | STATX_SIZE | STATX_BLOCKS;
    let requested = if _mask == 0 { STATX_BASIC_STATS } else { _mask };
    let out_mask = requested & STATX_BASIC_STATS;

    let node = if path.is_empty() {
        if (flags & AT_EMPTY_PATH) == 0 {
            return Err(EINVAL);
        }
        if _dirfd == AT_FDCWD {
            vfs::resolve_path(&vfs::current_dir())?
        } else {
            if _dirfd < 0 {
                return Err(crate::syscall::EBADF);
            }
            let f = fdtable::get_file(_dirfd as u32)?;
            f.lock().inode.clone().ok_or(EINVAL)?
        }
    } else {
        let full_path = resolve_at_path(_dirfd, path)?;
        if (flags & AT_SYMLINK_NOFOLLOW) != 0 {
            vfs::resolve_path_lstat(&full_path)?
        } else {
            vfs::resolve_path(&full_path)?
        }
    };
    let n = node.lock();

    let mut out = [0u8; 256];
    let type_bits: u16 = match n.itype {
        vfs::InodeType::File => 0o100000,
        vfs::InodeType::Directory => 0o040000,
        vfs::InodeType::CharDevice => 0o020000,
        vfs::InodeType::BlockDevice => 0o060000,
        vfs::InodeType::Pipe => 0o010000,
        vfs::InodeType::Socket => 0o140000,
        vfs::InodeType::Symlink => 0o120000,
    };
    let mode = type_bits | (n.mode as u16);

    // Minimal struct statx fields.
    out[0..4].copy_from_slice(&out_mask.to_le_bytes()); // stx_mask
    if (out_mask & STATX_BASIC_STATS) != 0 {
        out[4..8].copy_from_slice(&4096u32.to_le_bytes()); // stx_blksize
    }
    if (out_mask & (STATX_TYPE | STATX_MODE)) != 0 {
        out[28..30].copy_from_slice(&mode.to_le_bytes()); // stx_mode
    }
    if (out_mask & STATX_UID) != 0 {
        out[20..24].copy_from_slice(&n.uid.to_le_bytes()); // stx_uid
    }
    if (out_mask & STATX_GID) != 0 {
        out[24..28].copy_from_slice(&n.gid.to_le_bytes()); // stx_gid
    }
    if (out_mask & STATX_INO) != 0 {
        out[32..40].copy_from_slice(&1u64.to_le_bytes()); // stx_ino
    }
    if (out_mask & STATX_SIZE) != 0 {
        out[40..48].copy_from_slice(&(n.size as u64).to_le_bytes()); // stx_size
    }
    if (out_mask & STATX_BLOCKS) != 0 {
        out[48..56].copy_from_slice(&1u64.to_le_bytes()); // stx_blocks
    }

    crate::syscall::fs::copy_to_user(statxbuf, &out).map_err(|_| crate::syscall::EFAULT)?;
    Ok(0)
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
    const F_DUPFD: u32 = 0;
    const F_GETFD: u32 = 1;
    const F_SETFD: u32 = 2;
    const F_GETFL: u32 = 3;
    const F_SETFL: u32 = 4;
    const F_DUPFD_CLOEXEC: u32 = 1030;
    const FD_CLOEXEC: u64 = 1;
    match cmd {
        F_DUPFD => {
            let start = arg as usize;
            let file = fdtable::get_file(fd)?;
            let newfd = fdtable::alloc_fd_from_cloexec(start.max(3), file.clone(), false)?;
            if file.lock().ftype == vfs::FileType::Socket {
                crate::net::clone_socket_fd(fd as i32, newfd as i32);
            }
            Ok(newfd)
        }
        F_DUPFD_CLOEXEC => {
            let start = arg as usize;
            let file = fdtable::get_file(fd)?;
            let newfd = fdtable::alloc_fd_from_cloexec(start.max(3), file.clone(), true)?;
            if file.lock().ftype == vfs::FileType::Socket {
                crate::net::clone_socket_fd(fd as i32, newfd as i32);
            }
            Ok(newfd)
        }
        F_GETFD => Ok(if fdtable::get_cloexec(fd)? { FD_CLOEXEC as usize } else { 0 }),
        F_SETFD => fdtable::set_cloexec(fd, (arg & FD_CLOEXEC) != 0),
        F_GETFL => {
            let file = fdtable::get_file(fd)?;
            Ok(file.lock().flags as usize)
        }
        F_SETFL => {
            let file = fdtable::get_file(fd)?;
            file.lock().flags = arg as u32;
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

pub fn do_ioctl(fd: u32, request: u64, arg: u64) -> SyscallResult {
    let file = fdtable::get_file(fd)?;
    file.lock().ioctl(request, arg)
}

pub fn epoll_create1(flags: u32) -> SyscallResult {
    if (flags & !EPOLL_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let file = Arc::new(Mutex::new(vfs::FileObject::new_epoll()));
    let epfd = fdtable::alloc_fd(file)? as i32;
    EPOLL_TABLE.lock().entry(epfd).or_insert_with(BTreeMap::new);
    Ok(epfd as usize)
}

pub fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: u64) -> SyscallResult {
    if epfd < 0 || fd < 0 || epfd == fd {
        return Err(EINVAL);
    }
    let ep_file = fdtable::get_file(epfd as u32)?;
    if ep_file.lock().ftype != vfs::FileType::Epoll {
        return Err(EINVAL);
    }
    let _ = fdtable::get_file(fd as u32)?;

    let mut epolls = EPOLL_TABLE.lock();
    let watches = epolls.get_mut(&epfd).ok_or(EBADF)?;
    match op {
        EPOLL_CTL_ADD | EPOLL_CTL_MOD => {
            if event == 0 {
                return Err(EINVAL);
            }
            let mut raw = [0u8; 16];
            crate::syscall::fs::copy_from_user(event, &mut raw).map_err(|_| crate::syscall::EFAULT)?;
            let events = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
            let data = u64::from_le_bytes([
                raw[8], raw[9], raw[10], raw[11], raw[12], raw[13], raw[14], raw[15],
            ]);
            if op == EPOLL_CTL_ADD {
                if watches.contains_key(&fd) {
                    return Err(EEXIST);
                }
                watches.insert(fd, EpollWatch { events, data });
            } else {
                let w = watches.get_mut(&fd).ok_or(ENOENT)?;
                *w = EpollWatch { events, data };
            }
            Ok(0)
        }
        EPOLL_CTL_DEL => {
            if watches.remove(&fd).is_none() {
                return Err(ENOENT);
            }
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

pub fn epoll_pwait(epfd: i32, events: u64, maxevents: i32, timeout: i32) -> SyscallResult {
    if epfd < 0 || maxevents <= 0 || events == 0 {
        return Err(EINVAL);
    }
    if timeout < -1 {
        return Err(EINVAL);
    }
    let ep_file = fdtable::get_file(epfd as u32)?;
    if ep_file.lock().ftype != vfs::FileType::Epoll {
        return Err(EINVAL);
    }

    fn ready_mask_for_fd(fd: i32, interest: u32) -> u32 {
        let mut ready = 0u32;
        let Ok(f) = crate::fs::fdtable::get_file(fd as u32) else {
            return EPOLLHUP;
        };
        let file = f.lock();
        match file.ftype {
            vfs::FileType::Regular
            | vfs::FileType::Directory
            | vfs::FileType::CharDevice
            | vfs::FileType::MemFd
            | vfs::FileType::PidFd => {
                ready |= EPOLLIN | EPOLLOUT;
            }
            vfs::FileType::Pipe => {
                if let Some(vfs::SpecialData::PipeBuffer(ref pipe_buf)) = file.special_data {
                    if !pipe_buf.lock().is_empty() {
                        ready |= EPOLLIN;
                    }
                }
                ready |= EPOLLOUT;
            }
            vfs::FileType::EventFd => {
                if let Some(vfs::SpecialData::EventFdVal(v)) = file.special_data {
                    if v > 0 {
                        ready |= EPOLLIN;
                    }
                }
                ready |= EPOLLOUT;
            }
            vfs::FileType::TimerFd => {
                if let Some(vfs::SpecialData::TimerFdState { next_deadline, pending_expirations, .. }) = file.special_data {
                    if pending_expirations > 0 || (next_deadline != 0 && crate::arch::read_counter() >= next_deadline) {
                        ready |= EPOLLIN;
                    }
                }
            }
            vfs::FileType::SignalFd => {
                if let Some(vfs::SpecialData::SignalFdMask(mask)) = file.special_data {
                    if crate::ipc::signal::signalfd_ready(mask) {
                        ready |= EPOLLIN;
                    }
                }
            }
            vfs::FileType::Socket => {
                ready |= crate::net::socket_poll_mask(fd) & (EPOLLIN | EPOLLOUT | EPOLLERR | EPOLLHUP);
            }
            vfs::FileType::PtyMaster | vfs::FileType::PtySlave => {
                if let Some(sd) = &file.special_data {
                    let (id, is_master) = match sd {
                        vfs::SpecialData::PtyMaster(id) => (*id, true),
                        vfs::SpecialData::PtySlave(id) => (*id, false),
                        _ => (u32::MAX, true),
                    };
                    if id != u32::MAX {
                        if let Some(pty_arc) = crate::drivers::pty::get_pty(id) {
                            let pty = pty_arc.lock();
                            let has_in = if is_master {
                                !pty.master_rx.lock().is_empty()
                            } else {
                                !pty.slave_rx.lock().is_empty()
                            };
                            if has_in {
                                ready |= EPOLLIN;
                            }
                        }
                    }
                }
                ready |= EPOLLOUT;
            }
            vfs::FileType::Epoll => {}
        }
        ready & (interest | EPOLLERR | EPOLLHUP)
    }

    let freq = crate::arch::counter_freq().max(1);
    let deadline = if timeout > 0 {
        let ticks = (timeout as u64).saturating_mul(freq) / 1000;
        Some(crate::arch::read_counter().saturating_add(ticks))
    } else {
        None
    };

    loop {
        let snapshot = EPOLL_TABLE.lock().get(&epfd).cloned().unwrap_or_default();
        let mut ready_list: Vec<[u8; 16]> = Vec::new();
        for (fd, watch) in snapshot {
            let mask = ready_mask_for_fd(fd, watch.events);
            if mask == 0 {
                continue;
            }
            let mut rec = [0u8; 16];
            rec[0..4].copy_from_slice(&mask.to_le_bytes());
            rec[8..16].copy_from_slice(&watch.data.to_le_bytes());
            ready_list.push(rec);
            if ready_list.len() >= maxevents as usize {
                break;
            }
        }

        if !ready_list.is_empty() {
            for (i, rec) in ready_list.iter().enumerate() {
                let dst = events + (i * 16) as u64;
                crate::syscall::fs::copy_to_user(dst, rec).map_err(|_| crate::syscall::EFAULT)?;
            }
            return Ok(ready_list.len());
        }

        if timeout == 0 {
            return Ok(0);
        }
        if let Some(dl) = deadline {
            if crate::arch::read_counter() >= dl {
                return Ok(0);
            }
        }
        crate::sched::wait::sleep_ticks(1);
        core::hint::spin_loop();
    }
}

pub fn eventfd2(initval: u32, flags: u32) -> SyscallResult {
    let file = Arc::new(Mutex::new(vfs::FileObject::new_eventfd(initval)));
    fdtable::alloc_fd(file)
}

fn timespec_to_ticks(sec: u64, nsec: u64) -> Result<u64, i32> {
    if nsec >= 1_000_000_000 {
        return Err(EINVAL);
    }
    let freq = crate::arch::counter_freq().max(1);
    Ok(sec.saturating_mul(freq).saturating_add(nsec.saturating_mul(freq) / 1_000_000_000))
}

fn ticks_to_timespec(ticks: u64) -> (u64, u64) {
    let freq = crate::arch::counter_freq().max(1);
    let sec = ticks / freq;
    let rem = ticks % freq;
    let nsec = rem.saturating_mul(1_000_000_000) / freq;
    (sec, nsec)
}

pub fn timerfd_create(flags: u32) -> SyscallResult {
    const TFD_CLOEXEC: u32 = 0x80000;
    const TFD_NONBLOCK: u32 = 0x800;
    if (flags & !(TFD_CLOEXEC | TFD_NONBLOCK)) != 0 {
        return Err(EINVAL);
    }
    let mut obj = vfs::FileObject::new_timerfd();
    if (flags & TFD_NONBLOCK) != 0 {
        obj.flags |= 0o4000;
    }
    let fd = fdtable::alloc_fd(Arc::new(Mutex::new(obj)))?;
    if (flags & TFD_CLOEXEC) != 0 {
        let _ = fdtable::set_cloexec(fd as u32, true);
    }
    Ok(fd)
}

pub fn timerfd_settime(fd: i32, flags: i32, new_value: u64, old_value: u64) -> SyscallResult {
    const TFD_TIMER_ABSTIME: i32 = 1;
    if fd < 0 || new_value == 0 {
        return Err(EINVAL);
    }
    if (flags & !TFD_TIMER_ABSTIME) != 0 {
        return Err(EINVAL);
    }
    let file = fdtable::get_file(fd as u32)?;
    let mut fo = file.lock();
    if fo.ftype != vfs::FileType::TimerFd {
        return Err(EBADF);
    }

    let now = crate::arch::read_counter();
    let mut new_raw = [0u8; 32];
    crate::syscall::fs::copy_from_user(new_value, &mut new_raw).map_err(|_| crate::syscall::EFAULT)?;
    let int_sec = u64::from_le_bytes(new_raw[0..8].try_into().unwrap_or([0; 8]));
    let int_nsec = u64::from_le_bytes(new_raw[8..16].try_into().unwrap_or([0; 8]));
    let val_sec = u64::from_le_bytes(new_raw[16..24].try_into().unwrap_or([0; 8]));
    let val_nsec = u64::from_le_bytes(new_raw[24..32].try_into().unwrap_or([0; 8]));
    let interval_ticks = timespec_to_ticks(int_sec, int_nsec)?;
    let value_ticks = timespec_to_ticks(val_sec, val_nsec)?;

    if old_value != 0 {
        let mut old_out = [0u8; 32];
        if let Some(vfs::SpecialData::TimerFdState { next_deadline, interval_ticks, .. }) = fo.special_data {
            let remaining = if next_deadline > now { next_deadline - now } else { 0 };
            let (rsec, rnsec) = ticks_to_timespec(remaining);
            let (isec, insec) = ticks_to_timespec(interval_ticks);
            old_out[0..8].copy_from_slice(&isec.to_le_bytes());
            old_out[8..16].copy_from_slice(&insec.to_le_bytes());
            old_out[16..24].copy_from_slice(&rsec.to_le_bytes());
            old_out[24..32].copy_from_slice(&rnsec.to_le_bytes());
        }
        crate::syscall::fs::copy_to_user(old_value, &old_out).map_err(|_| crate::syscall::EFAULT)?;
    }

    if let Some(vfs::SpecialData::TimerFdState {
        ref mut next_deadline,
        interval_ticks: ref mut state_interval,
        ref mut pending_expirations,
    }) = fo.special_data
    {
        *state_interval = interval_ticks;
        *pending_expirations = 0;
        if value_ticks == 0 {
            *next_deadline = 0;
        } else if (flags & TFD_TIMER_ABSTIME) != 0 {
            *next_deadline = value_ticks;
            if *next_deadline <= now {
                *pending_expirations = 1;
                if *state_interval > 0 {
                    let delta = now.saturating_sub(*next_deadline);
                    let periods = delta / *state_interval + 1;
                    *next_deadline = next_deadline.saturating_add(periods.saturating_mul(*state_interval));
                }
            }
        } else {
            *next_deadline = now.saturating_add(value_ticks);
        }
    }
    Ok(0)
}

pub fn signalfd4(fd: i32, mask: u64, sizemask: usize, flags: i32) -> SyscallResult {
    const SFD_CLOEXEC: i32 = 0x80000;
    const SFD_NONBLOCK: i32 = 0x800;
    if sizemask != 8 {
        return Err(EINVAL);
    }
    if (flags & !(SFD_CLOEXEC | SFD_NONBLOCK)) != 0 {
        return Err(EINVAL);
    }
    let mut raw = [0u8; 8];
    crate::syscall::fs::copy_from_user(mask, &mut raw).map_err(|_| crate::syscall::EFAULT)?;
    let sigmask = u64::from_le_bytes(raw);

    if fd >= 0 {
        let file = fdtable::get_file(fd as u32)?;
        let mut fo = file.lock();
        if fo.ftype != vfs::FileType::SignalFd {
            return Err(EBADF);
        }
        fo.special_data = Some(vfs::SpecialData::SignalFdMask(sigmask));
        if (flags & SFD_NONBLOCK) != 0 {
            fo.flags |= 0o4000;
        } else {
            fo.flags &= !0o4000;
        }
        let _ = fdtable::set_cloexec(fd as u32, (flags & SFD_CLOEXEC) != 0);
        return Ok(fd as usize);
    }

    let mut obj = vfs::FileObject::new_signalfd(sigmask);
    if (flags & SFD_NONBLOCK) != 0 {
        obj.flags |= 0o4000;
    }
    let newfd = fdtable::alloc_fd(Arc::new(Mutex::new(obj)))?;
    if (flags & SFD_CLOEXEC) != 0 {
        let _ = fdtable::set_cloexec(newfd as u32, true);
    }
    Ok(newfd)
}

pub fn memfd_create(flags: u32) -> SyscallResult {
    let file = Arc::new(Mutex::new(vfs::FileObject::new_memfd()));
    fdtable::alloc_fd(file)
}
