use super::SyscallResult;

pub fn sys_socket(domain: i32, sock_type: i32, protocol: i32) -> SyscallResult {
    crate::net::do_socket(domain, sock_type, protocol)
}

pub fn sys_socketpair(domain: i32, sock_type: i32, protocol: i32, sv: u64) -> SyscallResult {
    crate::net::do_socketpair(domain, sock_type, protocol, sv)
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

pub fn sys_sendmsg(fd: i32, msg: u64, flags: u32) -> SyscallResult {
    crate::net::do_sendmsg(fd, msg, flags)
}

pub fn sys_recvmsg(fd: i32, msg: u64, flags: u32) -> SyscallResult {
    crate::net::do_recvmsg(fd, msg, flags)
}

pub fn sys_sendmmsg(fd: i32, mmsg: u64, vlen: u32, flags: u32) -> SyscallResult {
    if mmsg == 0 || vlen == 0 {
        return Err(crate::syscall::EINVAL);
    }
    let mut sent = 0usize;
    let capped = core::cmp::min(vlen as usize, 64);
    for i in 0..capped {
        let base = mmsg + (i as u64) * 64;
        match crate::net::do_sendmsg(fd, base, flags) {
            Ok(n) => {
                let len = n as u32;
                crate::syscall::fs::copy_to_user(base + 56, &len.to_le_bytes()).map_err(|_| crate::syscall::EFAULT)?;
                sent += 1;
            }
            Err(e) => {
                if sent > 0 {
                    return Ok(sent);
                }
                return Err(e);
            }
        }
    }
    Ok(sent)
}

pub fn sys_recvmmsg(fd: i32, mmsg: u64, vlen: u32, flags: u32, _timeout: u64) -> SyscallResult {
    if mmsg == 0 || vlen == 0 {
        return Err(crate::syscall::EINVAL);
    }
    let mut recvd = 0usize;
    let capped = core::cmp::min(vlen as usize, 64);
    for i in 0..capped {
        let base = mmsg + (i as u64) * 64;
        match crate::net::do_recvmsg(fd, base, flags) {
            Ok(n) => {
                let len = n as u32;
                crate::syscall::fs::copy_to_user(base + 56, &len.to_le_bytes()).map_err(|_| crate::syscall::EFAULT)?;
                recvd += 1;
            }
            Err(e) => {
                if recvd > 0 {
                    return Ok(recvd);
                }
                return Err(e);
            }
        }
    }
    Ok(recvd)
}

pub fn sys_getsockname(fd: i32, addr: u64, addrlen_ptr: u64) -> SyscallResult {
    crate::net::do_getsockname(fd, addr, addrlen_ptr)
}

pub fn sys_getpeername(fd: i32, addr: u64, addrlen_ptr: u64) -> SyscallResult {
    crate::net::do_getpeername(fd, addr, addrlen_ptr)
}

pub fn sys_setsockopt(fd: i32, level: i32, optname: i32, optval: u64, optlen: u32) -> SyscallResult {
    crate::net::do_setsockopt(fd, level, optname, optval, optlen)
}

pub fn sys_getsockopt(fd: i32, level: i32, optname: i32, optval: u64, optlen: u64) -> SyscallResult {
    crate::net::do_getsockopt(fd, level, optname, optval, optlen)
}
