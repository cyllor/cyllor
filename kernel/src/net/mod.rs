use crate::syscall::{SyscallResult, ENOSYS, EAFNOSUPPORT, ENOTSOCK};

// Stub network implementation
// Will be replaced with smoltcp in Phase 7

pub fn do_socket(domain: i32, sock_type: i32, protocol: i32) -> SyscallResult {
    // Create a socket fd
    let file = alloc::sync::Arc::new(spin::Mutex::new(
        crate::fs::vfs::FileObject {
            inode: None,
            offset: 0,
            flags: 0,
            ftype: crate::fs::vfs::FileType::Socket,
            special_data: None,
        }
    ));
    crate::fs::alloc_fd(file)
}

pub fn do_bind(fd: i32, addr: u64, addrlen: u32) -> SyscallResult {
    Ok(0)
}

pub fn do_listen(fd: i32, backlog: i32) -> SyscallResult {
    Ok(0)
}

pub fn do_accept4(fd: i32, addr: u64, addrlen: u64, flags: u32) -> SyscallResult {
    Err(ENOSYS) // Would block forever - not implemented yet
}

pub fn do_connect(fd: i32, addr: u64, addrlen: u32) -> SyscallResult {
    Err(crate::syscall::ECONNREFUSED)
}

pub fn do_sendto(fd: i32, buf: u64, len: u64, flags: u32, dest_addr: u64, addrlen: u32) -> SyscallResult {
    Ok(len as usize) // Pretend we sent it
}

pub fn do_recvfrom(fd: i32, buf: u64, len: u64, flags: u32, src_addr: u64, addrlen: u64) -> SyscallResult {
    Ok(0) // No data
}
