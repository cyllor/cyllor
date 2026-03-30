use super::{SyscallResult, ENOSYS, ECHILD};

pub fn sys_exit(code: i32) -> SyscallResult {
    log::debug!("Process exit with code {code}");
    // TODO: actually kill the process
    loop { core::hint::spin_loop(); }
}

pub fn sys_exit_group(code: i32) -> SyscallResult {
    sys_exit(code)
}

pub fn sys_getpid() -> SyscallResult {
    // TODO: get actual PID from current process
    Ok(1)
}

pub fn sys_gettid() -> SyscallResult {
    // TODO: get actual TID
    Ok(1)
}

pub fn sys_clone(flags: u64, stack: u64, ptid: u64, tls: u64, ctid: u64) -> SyscallResult {
    crate::sched::do_clone(flags, stack, ptid, tls, ctid)
}

pub fn sys_execve(pathname: u64, argv: u64, envp: u64) -> SyscallResult {
    crate::sched::do_execve(pathname, argv, envp)
}

pub fn sys_wait4(pid: i32, wstatus: u64, options: u32, rusage: u64) -> SyscallResult {
    crate::sched::do_wait4(pid, wstatus, options, rusage)
}

pub fn sys_set_tid_address(tidptr: u64) -> SyscallResult {
    // Store the TID pointer, return current TID
    Ok(1)
}

pub fn sys_prlimit64(pid: i32, resource: u32, new_limit: u64, old_limit: u64) -> SyscallResult {
    // Stub: return reasonable defaults
    if old_limit != 0 {
        // struct rlimit { rlim_cur, rlim_max } = 2 * u64
        unsafe {
            *(old_limit as *mut u64) = u64::MAX; // soft
            *((old_limit + 8) as *mut u64) = u64::MAX; // hard
        }
    }
    Ok(0)
}
