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
    let mut data = [0u8; 65 * 6];
    let write_field = |data: &mut [u8], offset: usize, val: &[u8]| {
        data[offset..offset + val.len()].copy_from_slice(val);
    };
    write_field(&mut data, 0, b"Cyllor");
    write_field(&mut data, 65, b"cyllor");
    write_field(&mut data, 130, b"6.1.0");    // Pretend Linux 6.1
    write_field(&mut data, 195, b"Cyllor OS 0.1");
    write_field(&mut data, 260, crate::arch::ARCH_NAME.as_bytes());
    super::fs::copy_to_user(buf, &data).map_err(|_| super::EFAULT)?;
    Ok(0)
}

pub fn sys_statfs(fd_or_path: u64, buf: u64) -> SyscallResult {
    if buf != 0 {
        let mut data = [0u8; 120];
        data[0..8].copy_from_slice(&0xEF53u64.to_le_bytes());
        data[8..16].copy_from_slice(&4096u64.to_le_bytes());
        data[16..24].copy_from_slice(&(1024 * 1024u64).to_le_bytes());
        data[24..32].copy_from_slice(&(512 * 1024u64).to_le_bytes());
        data[32..40].copy_from_slice(&(512 * 1024u64).to_le_bytes());
        data[56..64].copy_from_slice(&255u64.to_le_bytes());
        let _ = super::fs::copy_to_user(buf, &data);
    }
    Ok(0)
}

pub fn sys_prlimit64(pid: i32, resource: u32, new_limit: u64, old_limit: u64) -> SyscallResult {
    if old_limit != 0 {
        let mut data = [0u8; 16];
        data[0..8].copy_from_slice(&u64::MAX.to_le_bytes());
        data[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
        let _ = super::fs::copy_to_user(old_limit, &data);
    }
    Ok(0)
}
