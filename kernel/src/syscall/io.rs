use super::{SyscallResult, ENOSYS, EINVAL, EBADF};

pub fn sys_ioctl(fd: u32, request: u64, arg: u64) -> SyscallResult {
    crate::fs::do_ioctl(fd, request, arg)
}

pub fn sys_ppoll(fds: u64, nfds: u32, timeout: u64, sigmask: u64) -> SyscallResult {
    // Simple poll implementation - check fds and return immediately
    // For now, return 0 (timeout) to avoid blocking
    Ok(0)
}

pub fn sys_epoll_create1(flags: u32) -> SyscallResult {
    crate::fs::epoll_create1(flags)
}

pub fn sys_epoll_ctl(epfd: i32, op: i32, fd: i32, event: u64) -> SyscallResult {
    crate::fs::epoll_ctl(epfd, op, fd, event)
}

pub fn sys_epoll_pwait(epfd: i32, events: u64, maxevents: i32, timeout: i32, sigmask: u64) -> SyscallResult {
    crate::fs::epoll_pwait(epfd, events, maxevents, timeout)
}

pub fn sys_eventfd2(initval: u32, flags: u32) -> SyscallResult {
    crate::fs::eventfd2(initval, flags)
}

pub fn sys_futex(uaddr: u64, futex_op: i32, val: u32, timeout: u64, uaddr2: u64, val3: u32) -> SyscallResult {
    crate::ipc::futex::do_futex(uaddr, futex_op, val, timeout, uaddr2, val3)
}
