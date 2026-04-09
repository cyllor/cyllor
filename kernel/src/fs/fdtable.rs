use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use super::vfs::FileObject;
use crate::syscall::{EBADF, EMFILE, SyscallResult};

const MAX_FDS: usize = 1024;

static FD_TABLE: Mutex<FdTable> = Mutex::new(FdTable::new());

struct FdTable {
    files: Vec<Option<Arc<Mutex<FileObject>>>>,
    cloexec: Vec<bool>,
}

impl FdTable {
    const fn new() -> Self {
        Self {
            files: Vec::new(),
            cloexec: Vec::new(),
        }
    }

    fn ensure_init(&mut self) {
        if self.files.is_empty() {
            self.files.resize_with(MAX_FDS, || None);
            self.cloexec.resize(MAX_FDS, false);
            // FDs 0, 1, 2 are stdin, stdout, stderr — connect to console PTY slave
            let pty_id = crate::drivers::pty::console_pty_id();
            if pty_id != u32::MAX {
                for fd in 0..3 {
                    self.files[fd] = Some(Arc::new(Mutex::new(FileObject {
                        inode: None,
                        offset: 0,
                        flags: 0,
                        ftype: super::vfs::FileType::PtySlave,
                        special_data: Some(super::vfs::SpecialData::PtySlave(pty_id)),
                    })));
                }
            } else {
                // Fallback: bare char devices (before PTY init)
                for fd in 0..3 {
                    self.files[fd] = Some(Arc::new(Mutex::new(FileObject {
                        inode: None,
                        offset: 0,
                        flags: 0,
                        ftype: super::vfs::FileType::CharDevice,
                        special_data: None,
                    })));
                }
            }
        }
    }
}

pub fn get_file(fd: u32) -> Result<Arc<Mutex<FileObject>>, i32> {
    let mut table = FD_TABLE.lock();
    table.ensure_init();
    let fd = fd as usize;
    if fd >= MAX_FDS {
        return Err(EBADF);
    }
    table.files[fd].clone().ok_or(EBADF)
}

pub fn alloc_fd(file: Arc<Mutex<FileObject>>) -> SyscallResult {
    alloc_fd_from(3, file)
}

pub fn alloc_fd_from(start: usize, file: Arc<Mutex<FileObject>>) -> SyscallResult {
    alloc_fd_from_cloexec(start, file, false)
}

pub fn alloc_fd_from_cloexec(start: usize, file: Arc<Mutex<FileObject>>, cloexec: bool) -> SyscallResult {
    let mut table = FD_TABLE.lock();
    table.ensure_init();
    let begin = start.min(MAX_FDS);
    for i in begin..MAX_FDS {
        if table.files[i].is_none() {
            table.files[i] = Some(file);
            table.cloexec[i] = cloexec;
            return Ok(i);
        }
    }
    Err(EMFILE)
}

pub fn close_fd(fd: u32) -> SyscallResult {
    let mut table = FD_TABLE.lock();
    table.ensure_init();
    let fd = fd as usize;
    if fd >= MAX_FDS {
        return Err(EBADF);
    }
    if table.files[fd].is_some() {
        if let Some(ref file) = table.files[fd] {
            if file.lock().ftype == super::vfs::FileType::Socket {
                crate::net::cleanup_socket_fd(fd as i32);
            }
        }
        table.files[fd] = None;
        table.cloexec[fd] = false;
        Ok(0)
    } else {
        Err(EBADF)
    }
}

pub fn dup_fd(oldfd: u32) -> SyscallResult {
    let mut table = FD_TABLE.lock();
    table.ensure_init();
    let old = oldfd as usize;
    if old >= MAX_FDS {
        return Err(EBADF);
    }
    let file = table.files[old].clone().ok_or(EBADF)?;
    for i in 3..MAX_FDS {
        if table.files[i].is_none() {
            if file.lock().ftype == super::vfs::FileType::Socket {
                crate::net::clone_socket_fd(old as i32, i as i32);
            }
            table.files[i] = Some(file);
            table.cloexec[i] = false;
            return Ok(i);
        }
    }
    Err(EMFILE)
}

pub fn dup3_fd(oldfd: u32, newfd: u32, flags: u32) -> SyscallResult {
    if oldfd == newfd {
        return Err(crate::syscall::EINVAL);
    }
    if flags != 0 {
        // Only minimal dup3 semantics are supported for now.
        return Err(crate::syscall::EINVAL);
    }
    let file = get_file(oldfd)?;
    let mut table = FD_TABLE.lock();
    table.ensure_init();
    let newfd = newfd as usize;
    if newfd >= MAX_FDS {
        return Err(EBADF);
    }
    if let Some(ref old_target) = table.files[newfd] {
        if old_target.lock().ftype == super::vfs::FileType::Socket {
            crate::net::cleanup_socket_fd(newfd as i32);
        }
    }
    if file.lock().ftype == super::vfs::FileType::Socket {
        crate::net::clone_socket_fd(oldfd as i32, newfd as i32);
    }
    table.files[newfd] = Some(file);
    table.cloexec[newfd] = false;
    Ok(newfd)
}

pub fn get_cloexec(fd: u32) -> Result<bool, i32> {
    let mut table = FD_TABLE.lock();
    table.ensure_init();
    let fd = fd as usize;
    if fd >= MAX_FDS {
        return Err(EBADF);
    }
    if table.files[fd].is_none() {
        return Err(EBADF);
    }
    Ok(table.cloexec[fd])
}

pub fn set_cloexec(fd: u32, val: bool) -> SyscallResult {
    let mut table = FD_TABLE.lock();
    table.ensure_init();
    let fd = fd as usize;
    if fd >= MAX_FDS {
        return Err(EBADF);
    }
    if table.files[fd].is_none() {
        return Err(EBADF);
    }
    table.cloexec[fd] = val;
    Ok(0)
}

pub fn close_cloexec_fds() {
    let fds: Vec<u32> = {
        let mut table = FD_TABLE.lock();
        table.ensure_init();
        table
            .files
            .iter()
            .enumerate()
            .filter_map(|(i, f)| {
                if f.is_some() && table.cloexec.get(i).copied().unwrap_or(false) {
                    Some(i as u32)
                } else {
                    None
                }
            })
            .collect()
    };
    for fd in fds {
        let _ = close_fd(fd);
    }
}

pub fn list_open_fds() -> Vec<u32> {
    let mut table = FD_TABLE.lock();
    table.ensure_init();
    table
        .files
        .iter()
        .enumerate()
        .filter_map(|(i, f)| if f.is_some() { Some(i as u32) } else { None })
        .collect()
}
