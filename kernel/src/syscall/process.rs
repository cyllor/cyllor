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

pub fn sys_prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> SyscallResult {
    match option {
        15 => Ok(0), // PR_SET_NAME - accept silently
        16 => Ok(0), // PR_GET_NAME
        38 => Ok(0), // PR_SET_NO_NEW_PRIVS
        _ => Ok(0),  // Accept all prctl options
    }
}

pub fn sys_uname(buf: u64) -> SyscallResult {
    if buf == 0 { return Err(super::EFAULT); }
    // struct utsname: 5 fields of 65 bytes each
    unsafe {
        core::ptr::write_bytes(buf as *mut u8, 0, 65 * 6);
        let write_field = |offset: usize, val: &[u8]| {
            core::ptr::copy_nonoverlapping(val.as_ptr(), (buf + offset as u64) as *mut u8, val.len());
        };
        write_field(0, b"Cyllor");           // sysname
        write_field(65, b"cyllor");          // nodename
        write_field(130, b"0.1.0");          // release
        write_field(195, b"Cyllor OS 0.1"); // version
        write_field(260, b"aarch64");        // machine
    }
    Ok(0)
}

pub fn sys_statfs(fd_or_path: u64, buf: u64) -> SyscallResult {
    if buf != 0 {
        unsafe {
            core::ptr::write_bytes(buf as *mut u8, 0, 120);
            // f_type = EXT4_SUPER_MAGIC
            *(buf as *mut u64) = 0xEF53;
            // f_bsize
            *((buf + 8) as *mut u64) = 4096;
            // f_blocks
            *((buf + 16) as *mut u64) = 1024 * 1024;
            // f_bfree
            *((buf + 24) as *mut u64) = 512 * 1024;
            // f_bavail
            *((buf + 32) as *mut u64) = 512 * 1024;
            // f_namelen
            *((buf + 56) as *mut u64) = 255;
        }
    }
    Ok(0)
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
