use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use super::vfs::FileObject;
use crate::syscall::{EBADF, EMFILE, SyscallResult};

const MAX_FDS: usize = 1024;

static FD_TABLE: Mutex<FdTable> = Mutex::new(FdTable::new());

struct FdTable {
    files: Vec<Option<Arc<Mutex<FileObject>>>>,
}

impl FdTable {
    const fn new() -> Self {
        Self { files: Vec::new() }
    }

    fn ensure_init(&mut self) {
        if self.files.is_empty() {
            self.files.resize_with(MAX_FDS, || None);
            // FDs 0, 1, 2 are stdin, stdout, stderr - create char device files
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
    let mut table = FD_TABLE.lock();
    table.ensure_init();
    for i in 3..MAX_FDS {
        if table.files[i].is_none() {
            table.files[i] = Some(file);
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
        table.files[fd] = None;
        Ok(0)
    } else {
        Err(EBADF)
    }
}

pub fn dup_fd(oldfd: u32) -> SyscallResult {
    let file = get_file(oldfd)?;
    alloc_fd(file)
}

pub fn dup3_fd(oldfd: u32, newfd: u32, flags: u32) -> SyscallResult {
    let file = get_file(oldfd)?;
    let mut table = FD_TABLE.lock();
    table.ensure_init();
    let newfd = newfd as usize;
    if newfd >= MAX_FDS {
        return Err(EBADF);
    }
    table.files[newfd] = Some(file);
    Ok(newfd)
}
