use super::vfs::FileObject;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::syscall::SyscallResult;

pub fn create_pipe() -> Result<(Arc<Mutex<FileObject>>, Arc<Mutex<FileObject>>), i32> {
    let buffer = Arc::new(Mutex::new(Vec::<u8>::with_capacity(4096)));

    let read_end = Arc::new(Mutex::new(FileObject::new_pipe(buffer.clone(), true)));
    let write_end = Arc::new(Mutex::new(FileObject::new_pipe(buffer, false)));

    Ok((read_end, write_end))
}
