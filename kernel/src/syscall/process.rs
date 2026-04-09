use super::{SyscallResult, ENOSYS, ECHILD};
use alloc::collections::BTreeMap;
use spin::Mutex;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CloneArgsRaw {
    flags: u64,
    pidfd: u64,
    child_tid: u64,
    parent_tid: u64,
    exit_signal: u64,
    stack: u64,
    stack_size: u64,
    tls: u64,
    set_tid: u64,
    set_tid_size: u64,
    cgroup: u64,
}

static CLEAR_CHILD_TID: Mutex<BTreeMap<u64, u64>> = Mutex::new(BTreeMap::new());
static ROBUST_LIST: Mutex<BTreeMap<u64, (u64, u64)>> = Mutex::new(BTreeMap::new()); // tid -> (head, len)
static RSEQ_STATE: Mutex<BTreeMap<u64, (u64, u64, u64)>> = Mutex::new(BTreeMap::new()); // tid -> (addr, len, sig)

pub fn register_clear_child_tid(tid: u64, tidptr: u64) {
    if tidptr != 0 {
        CLEAR_CHILD_TID.lock().insert(tid, tidptr);
    }
}

pub fn cleanup_thread_state(tid: u64) {
    CLEAR_CHILD_TID.lock().remove(&tid);
    ROBUST_LIST.lock().remove(&tid);
    RSEQ_STATE.lock().remove(&tid);
}

pub fn sys_exit(code: i32) -> SyscallResult {
    log::debug!("Process exit with code {code}");
    let pid = crate::sched::process::current_pid();
    crate::sched::note_process_exit(pid, code);
    if let Some(tidptr) = CLEAR_CHILD_TID.lock().remove(&pid) {
        if tidptr != 0 {
            let _ = super::fs::copy_to_user(tidptr, &0u32.to_le_bytes());
        }
    }
    ROBUST_LIST.lock().remove(&pid);

    crate::sched::scheduler::mark_current_dead();
    crate::sched::scheduler::schedule();
    loop {
        crate::sched::scheduler::block_current_until(u64::MAX);
        core::hint::spin_loop();
    }
}

pub fn sys_exit_group(code: i32) -> SyscallResult {
    let pid = crate::sched::process::current_pid();
    let (tgid, members) = {
        let table = crate::sched::process::PROCESS_TABLE.lock();
        let tgid = table.get(&pid).map(|p| p.tgid).unwrap_or(pid);
        let members = table
            .values()
            .filter(|p| p.tgid == tgid)
            .map(|p| p.pid)
            .collect::<alloc::vec::Vec<_>>();
        (tgid, members)
    };
    {
        let mut clear = CLEAR_CHILD_TID.lock();
        let mut robust = ROBUST_LIST.lock();
        for tid in &members {
            clear.remove(tid);
            robust.remove(tid);
        }
    }
    crate::sched::note_thread_group_exit(tgid, code);
    crate::sched::scheduler::mark_threads_dead(&members);
    crate::sched::scheduler::schedule();
    loop {
        crate::sched::scheduler::block_current_until(u64::MAX);
        core::hint::spin_loop();
    }
}

pub fn sys_getpid() -> SyscallResult {
    Ok(crate::sched::process::current_pid() as usize)
}

pub fn current_creds() -> (u32, u32, u32, u32) {
    let pid = crate::sched::process::current_pid();
    let table = crate::sched::process::PROCESS_TABLE.lock();
    if let Some(p) = table.get(&pid) {
        (p.uid, p.gid, p.euid, p.egid)
    } else {
        (0, 0, 0, 0)
    }
}

pub fn sys_getuid() -> SyscallResult {
    Ok(current_creds().0 as usize)
}

pub fn sys_getgid() -> SyscallResult {
    Ok(current_creds().1 as usize)
}

pub fn sys_geteuid() -> SyscallResult {
    Ok(current_creds().2 as usize)
}

pub fn sys_getegid() -> SyscallResult {
    Ok(current_creds().3 as usize)
}

pub fn sys_getppid() -> SyscallResult {
    let pid = crate::sched::process::current_pid();
    let table = crate::sched::process::PROCESS_TABLE.lock();
    let ppid = table.get(&pid).map(|p| p.ppid).unwrap_or(0);
    Ok(ppid as usize)
}

pub fn sys_gettid() -> SyscallResult {
    Ok(crate::sched::process::current_pid() as usize)
}

pub fn sys_getpgrp() -> SyscallResult {
    let pid = crate::sched::process::current_pid();
    let table = crate::sched::process::PROCESS_TABLE.lock();
    let pgrp = table.get(&pid).map(|p| p.pgid).unwrap_or(pid);
    Ok(pgrp as usize)
}

pub fn sys_getpgid(pid: i32) -> SyscallResult {
    let target = if pid == 0 {
        crate::sched::process::current_pid()
    } else if pid > 0 {
        pid as u64
    } else {
        return Err(super::EINVAL);
    };
    let table = crate::sched::process::PROCESS_TABLE.lock();
    let pgrp = table.get(&target).map(|p| p.pgid).ok_or(super::ESRCH)?;
    Ok(pgrp as usize)
}

pub fn sys_getsid(pid: i32) -> SyscallResult {
    let target = if pid == 0 {
        crate::sched::process::current_pid()
    } else if pid > 0 {
        pid as u64
    } else {
        return Err(super::EINVAL);
    };
    let table = crate::sched::process::PROCESS_TABLE.lock();
    let sid = table.get(&target).map(|p| p.sid).ok_or(super::ESRCH)?;
    Ok(sid as usize)
}

pub fn sys_setsid() -> SyscallResult {
    let pid = crate::sched::process::current_pid();
    let mut table = crate::sched::process::PROCESS_TABLE.lock();
    let proc = table.get_mut(&pid).ok_or(super::ESRCH)?;
    if proc.pgid == pid {
        return Err(super::EPERM);
    }
    proc.sid = pid;
    proc.pgid = pid;
    Ok(pid as usize)
}

pub fn sys_setpgid(pid: i32, pgid: i32) -> SyscallResult {
    if pgid < 0 {
        return Err(super::EINVAL);
    }
    let target = if pid == 0 {
        crate::sched::process::current_pid()
    } else if pid > 0 {
        pid as u64
    } else {
        return Err(super::EINVAL);
    };
    let mut table = crate::sched::process::PROCESS_TABLE.lock();
    let caller = crate::sched::process::current_pid();
    let caller_sid = table.get(&caller).map(|p| p.sid).ok_or(super::ESRCH)?;
    let new_pgid = if pgid == 0 { target } else { pgid as u64 };
    let target_sid = table.get(&target).map(|p| p.sid).ok_or(super::ESRCH)?;
    if target_sid != caller_sid {
        return Err(super::EPERM);
    }
    if new_pgid != target {
        let group_exists = table.values().any(|p| p.pgid == new_pgid && p.sid == caller_sid);
        if !group_exists {
            return Err(super::EPERM);
        }
    }
    let proc = table.get_mut(&target).ok_or(super::ESRCH)?;
    proc.pgid = new_pgid;
    Ok(0)
}

pub fn sys_setuid(uid: u32) -> SyscallResult {
    let pid = crate::sched::process::current_pid();
    let mut table = crate::sched::process::PROCESS_TABLE.lock();
    let p = table.get_mut(&pid).ok_or(super::ESRCH)?;
    if p.euid != 0 && uid != p.uid && uid != p.euid {
        return Err(super::EPERM);
    }
    p.uid = uid;
    p.euid = uid;
    p.suid = uid;
    Ok(0)
}

pub fn sys_setgid(gid: u32) -> SyscallResult {
    let pid = crate::sched::process::current_pid();
    let mut table = crate::sched::process::PROCESS_TABLE.lock();
    let p = table.get_mut(&pid).ok_or(super::ESRCH)?;
    if p.egid != 0 && gid != p.gid && gid != p.egid {
        return Err(super::EPERM);
    }
    p.gid = gid;
    p.egid = gid;
    p.sgid = gid;
    Ok(0)
}

pub fn sys_getresuid(ruid: u64, euid: u64, suid: u64) -> SyscallResult {
    let pid = crate::sched::process::current_pid();
    let table = crate::sched::process::PROCESS_TABLE.lock();
    let p = table.get(&pid).ok_or(super::ESRCH)?;
    if ruid != 0 {
        super::fs::copy_to_user(ruid, &p.uid.to_le_bytes()).map_err(|_| super::EFAULT)?;
    }
    if euid != 0 {
        super::fs::copy_to_user(euid, &p.euid.to_le_bytes()).map_err(|_| super::EFAULT)?;
    }
    if suid != 0 {
        super::fs::copy_to_user(suid, &p.suid.to_le_bytes()).map_err(|_| super::EFAULT)?;
    }
    Ok(0)
}

pub fn sys_getresgid(rgid: u64, egid: u64, sgid: u64) -> SyscallResult {
    let pid = crate::sched::process::current_pid();
    let table = crate::sched::process::PROCESS_TABLE.lock();
    let p = table.get(&pid).ok_or(super::ESRCH)?;
    if rgid != 0 {
        super::fs::copy_to_user(rgid, &p.gid.to_le_bytes()).map_err(|_| super::EFAULT)?;
    }
    if egid != 0 {
        super::fs::copy_to_user(egid, &p.egid.to_le_bytes()).map_err(|_| super::EFAULT)?;
    }
    if sgid != 0 {
        super::fs::copy_to_user(sgid, &p.sgid.to_le_bytes()).map_err(|_| super::EFAULT)?;
    }
    Ok(0)
}

pub fn sys_setresuid(ruid: i32, euid: i32, suid: i32) -> SyscallResult {
    let pid = crate::sched::process::current_pid();
    let mut table = crate::sched::process::PROCESS_TABLE.lock();
    let p = table.get_mut(&pid).ok_or(super::ESRCH)?;
    let is_root = p.euid == 0;
    let mut nr = p.uid;
    let mut ne = p.euid;
    let mut ns = p.suid;
    if ruid >= 0 {
        let v = ruid as u32;
        if !is_root && v != p.uid && v != p.euid && v != p.suid {
            return Err(super::EPERM);
        }
        nr = v;
    }
    if euid >= 0 {
        let v = euid as u32;
        if !is_root && v != p.uid && v != p.euid && v != p.suid {
            return Err(super::EPERM);
        }
        ne = v;
    }
    if suid >= 0 {
        let v = suid as u32;
        if !is_root && v != p.uid && v != p.euid && v != p.suid {
            return Err(super::EPERM);
        }
        ns = v;
    }
    p.uid = nr;
    p.euid = ne;
    p.suid = ns;
    Ok(0)
}

pub fn sys_setresgid(rgid: i32, egid: i32, sgid: i32) -> SyscallResult {
    let pid = crate::sched::process::current_pid();
    let mut table = crate::sched::process::PROCESS_TABLE.lock();
    let p = table.get_mut(&pid).ok_or(super::ESRCH)?;
    let is_root = p.euid == 0;
    let mut nr = p.gid;
    let mut ne = p.egid;
    let mut ns = p.sgid;
    if rgid >= 0 {
        let v = rgid as u32;
        if !is_root && v != p.gid && v != p.egid && v != p.sgid {
            return Err(super::EPERM);
        }
        nr = v;
    }
    if egid >= 0 {
        let v = egid as u32;
        if !is_root && v != p.gid && v != p.egid && v != p.sgid {
            return Err(super::EPERM);
        }
        ne = v;
    }
    if sgid >= 0 {
        let v = sgid as u32;
        if !is_root && v != p.gid && v != p.egid && v != p.sgid {
            return Err(super::EPERM);
        }
        ns = v;
    }
    p.gid = nr;
    p.egid = ne;
    p.sgid = ns;
    Ok(0)
}

pub fn sys_clone(frame: &mut impl crate::arch::CpuContext, flags: u64, stack: u64, ptid: u64, tls: u64, ctid: u64) -> SyscallResult {
    crate::sched::do_clone(frame, flags, stack, ptid, tls, ctid)
}

pub fn sys_clone3(frame: &mut impl crate::arch::CpuContext, clone_args: u64, size: u64) -> SyscallResult {
    if clone_args == 0 {
        return Err(super::EINVAL);
    }

    let mut raw = CloneArgsRaw::default();
    let to_copy = core::cmp::min(size as usize, core::mem::size_of::<CloneArgsRaw>());
    if to_copy < 64 {
        return Err(super::EINVAL);
    }

    let raw_bytes = unsafe {
        core::slice::from_raw_parts_mut((&mut raw as *mut CloneArgsRaw).cast::<u8>(), to_copy)
    };
    super::fs::copy_from_user(clone_args, raw_bytes).map_err(|_| super::EFAULT)?;

    if raw.exit_signal & !0xff != 0 {
        return Err(super::EINVAL);
    }
    if raw.set_tid_size != 0 && raw.set_tid == 0 {
        return Err(super::EINVAL);
    }

    // Unsupported clone3-only features for now.
    if raw.cgroup != 0 {
        return Err(super::EINVAL);
    }
    if raw.set_tid_size > 1 {
        return Err(super::EINVAL);
    }
    if raw.set_tid_size == 1 {
        let mut req_tid = [0u8; 4];
        super::fs::copy_from_user(raw.set_tid, &mut req_tid).map_err(|_| super::EFAULT)?;
        // We do not support explicit PID selection yet.
        if u32::from_le_bytes(req_tid) != 0 {
            return Err(super::EINVAL);
        }
    }

    // clone3 carries exit_signal separately; clone expects it in flags low byte.
    let mut flags = raw.flags | (raw.exit_signal & 0xff);
    if raw.pidfd != 0 {
        // CLONE_PIDFD: ask clone path to also return pidfd through parent_tid argument.
        flags |= 0x0000_1000; // CLONE_PIDFD
    }
    let parent_tid_ptr = if raw.pidfd != 0 { raw.pidfd } else { raw.parent_tid };
    sys_clone(frame, flags, raw.stack, parent_tid_ptr, raw.tls, raw.child_tid)
}

pub fn sys_execve(frame: &mut impl crate::arch::CpuContext, pathname: u64, argv: u64, envp: u64) -> SyscallResult {
    crate::sched::do_execve(frame, pathname, argv, envp)
}

pub fn sys_wait4(pid: i32, wstatus: u64, options: u32, rusage: u64) -> SyscallResult {
    crate::sched::do_wait4(pid, wstatus, options, rusage)
}

pub fn sys_waitid(idtype: i32, id: u32, infop: u64, options: u32, _rusage: u64) -> SyscallResult {
    const P_ALL: i32 = 0;
    const P_PID: i32 = 1;
    const WNOHANG: u32 = 1;
    const WEXITED: u32 = 0x0000_0004;
    const WNOWAIT: u32 = 0x0100_0000;

    if (options & WEXITED) == 0 {
        return Err(super::EINVAL);
    }
    if options & !(WNOHANG | WEXITED | WNOWAIT) != 0 {
        return Err(super::EINVAL);
    }

    let wait_pid = match idtype {
        P_ALL => -1,
        P_PID => id as i32,
        _ => return Err(super::EINVAL),
    };
    let (reaped, status) = crate::sched::do_wait4_with_status(wait_pid, options)?;
    if infop != 0 {
        let mut info = [0u8; 128];
        let si_signo = 17i32; // SIGCHLD
        let si_code = 1i32;   // CLD_EXITED
        let child_pid = reaped as i32;
        info[0..4].copy_from_slice(&si_signo.to_le_bytes());
        info[8..12].copy_from_slice(&si_code.to_le_bytes());
        // Fill both common offsets used by libc layouts.
        info[12..16].copy_from_slice(&child_pid.to_le_bytes());
        info[16..20].copy_from_slice(&child_pid.to_le_bytes());
        info[20..24].copy_from_slice(&0u32.to_le_bytes()); // uid
        info[24..28].copy_from_slice(&status.to_le_bytes());
        super::fs::copy_to_user(infop, &info).map_err(|_| super::EFAULT)?;
    }
    Ok(0)
}

pub fn sys_set_tid_address(tidptr: u64) -> SyscallResult {
    let tid = crate::sched::process::current_pid();
    CLEAR_CHILD_TID.lock().insert(tid, tidptr);
    Ok(tid as usize)
}

pub fn sys_rseq(rseq: u64, rseq_len: u64, flags: u64, sig: u64) -> SyscallResult {
    const RSEQ_FLAG_UNREGISTER: u64 = 1;
    const RSEQ_SIG: u64 = 0x5305_3053;

    if flags & !RSEQ_FLAG_UNREGISTER != 0 {
        return Err(super::EINVAL);
    }
    if rseq_len != 0 && rseq_len < 32 {
        return Err(super::EINVAL);
    }

    let tid = crate::sched::process::current_pid();
    if flags == RSEQ_FLAG_UNREGISTER {
        let mut map = RSEQ_STATE.lock();
        if let Some((old_addr, _, _)) = map.get(&tid).copied() {
            if rseq != 0 && rseq != old_addr {
                return Err(super::EINVAL);
            }
            map.remove(&tid);
            return Ok(0);
        }
        return Err(super::EINVAL);
    }
    if rseq == 0 {
        return Err(super::EINVAL);
    }
    if sig != 0 && sig != RSEQ_SIG {
        return Err(super::EINVAL);
    }

    // Minimal support: persist per-thread registration so unregister paths behave.
    RSEQ_STATE.lock().insert(tid, (rseq, rseq_len, sig));
    Ok(0)
}

pub fn sys_prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> SyscallResult {
    const PR_SET_NAME: i32 = 15;
    const PR_GET_NAME: i32 = 16;
    const PR_SET_NO_NEW_PRIVS: i32 = 38;
    const PR_GET_NO_NEW_PRIVS: i32 = 39;
    let _ = (arg3, arg4, arg5);
    match option {
        PR_SET_NAME => {
            if arg2 == 0 {
                return Err(super::EFAULT);
            }
            let pid = crate::sched::process::current_pid();
            let mut raw = [0u8; 16];
            super::fs::copy_from_user(arg2, &mut raw).map_err(|_| super::EFAULT)?;
            let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
            let name = core::str::from_utf8(&raw[..end]).map_err(|_| super::EINVAL)?;
            let mut table = crate::sched::process::PROCESS_TABLE.lock();
            if let Some(proc) = table.get_mut(&pid) {
                proc.name = alloc::string::String::from(name);
            }
            Ok(0)
        }
        PR_GET_NAME => {
            if arg2 == 0 {
                return Err(super::EFAULT);
            }
            let pid = crate::sched::process::current_pid();
            let table = crate::sched::process::PROCESS_TABLE.lock();
            let mut out = [0u8; 16];
            if let Some(proc) = table.get(&pid) {
                let bytes = proc.name.as_bytes();
                let n = core::cmp::min(bytes.len(), 15);
                out[..n].copy_from_slice(&bytes[..n]);
            }
            super::fs::copy_to_user(arg2, &out).map_err(|_| super::EFAULT)?;
            Ok(0)
        }
        PR_SET_NO_NEW_PRIVS => {
            if arg2 != 1 {
                return Err(super::EINVAL);
            }
            let pid = crate::sched::process::current_pid();
            let mut table = crate::sched::process::PROCESS_TABLE.lock();
            let proc = table.get_mut(&pid).ok_or(super::ESRCH)?;
            proc.no_new_privs = true;
            Ok(0)
        }
        PR_GET_NO_NEW_PRIVS => {
            let pid = crate::sched::process::current_pid();
            let table = crate::sched::process::PROCESS_TABLE.lock();
            let v = table.get(&pid).map(|p| p.no_new_privs).unwrap_or(false);
            Ok(if v { 1 } else { 0 })
        }
        _ => Err(super::EINVAL),
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

pub fn sys_set_robust_list(head: u64, len: u64) -> SyscallResult {
    if head == 0 || len == 0 {
        return Err(super::EINVAL);
    }
    let tid = crate::sched::process::current_pid();
    ROBUST_LIST.lock().insert(tid, (head, len));
    Ok(0)
}

pub fn sys_get_robust_list(pid: i32, head_ptr: u64, len_ptr: u64) -> SyscallResult {
    if head_ptr == 0 || len_ptr == 0 {
        return Err(super::EFAULT);
    }
    let caller = crate::sched::process::current_pid();
    let target = if pid == 0 { caller } else { pid as u64 };
    if !crate::sched::process::PROCESS_TABLE.lock().contains_key(&target) {
        return Err(super::ESRCH);
    }
    if target != caller {
        return Err(super::EPERM);
    }

    let (head, len) = ROBUST_LIST
        .lock()
        .get(&target)
        .copied()
        .unwrap_or((0, 0));

    super::fs::copy_to_user(head_ptr, &head.to_le_bytes()).map_err(|_| super::EFAULT)?;
    super::fs::copy_to_user(len_ptr, &len.to_le_bytes()).map_err(|_| super::EFAULT)?;
    Ok(0)
}
