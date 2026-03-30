use super::{SyscallResult, ENOSYS, EAFNOSUPPORT};

pub fn sys_socket(domain: i32, sock_type: i32, protocol: i32) -> SyscallResult {
    crate::net::do_socket(domain, sock_type, protocol)
}

pub fn sys_bind(fd: i32, addr: u64, addrlen: u32) -> SyscallResult {
    crate::net::do_bind(fd, addr, addrlen)
}

pub fn sys_listen(fd: i32, backlog: i32) -> SyscallResult {
    crate::net::do_listen(fd, backlog)
}

pub fn sys_accept4(fd: i32, addr: u64, addrlen: u64, flags: u32) -> SyscallResult {
    crate::net::do_accept4(fd, addr, addrlen, flags)
}

pub fn sys_connect(fd: i32, addr: u64, addrlen: u32) -> SyscallResult {
    crate::net::do_connect(fd, addr, addrlen)
}

pub fn sys_sendto(fd: i32, buf: u64, len: u64, flags: u32, dest_addr: u64, addrlen: u32) -> SyscallResult {
    crate::net::do_sendto(fd, buf, len, flags, dest_addr, addrlen)
}

pub fn sys_recvfrom(fd: i32, buf: u64, len: u64, flags: u32, src_addr: u64, addrlen: u64) -> SyscallResult {
    crate::net::do_recvfrom(fd, buf, len, flags, src_addr, addrlen)
}
