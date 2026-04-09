use crate::arch::CpuContext;
use crate::syscall::{SyscallResult, EINVAL, ENOSYS, ESRCH};
use alloc::collections::{BTreeMap, BTreeSet};
use spin::Mutex;

// pid/tid -> pending signal bitmap (signals 1..=64).
static PENDING_SIGNALS: Mutex<BTreeMap<u64, u64>> = Mutex::new(BTreeMap::new());
static BLOCKED_SIGNALS: Mutex<BTreeMap<u64, u64>> = Mutex::new(BTreeMap::new());
static IN_SIGNAL_HANDLER: Mutex<BTreeSet<u64>> = Mutex::new(BTreeSet::new());

#[derive(Clone, Copy, Default)]
struct KernelSigAction {
    handler: u64,
    flags: u64,
    restorer: u64,
    mask: u64,
}

#[derive(Clone)]
struct SavedUserContext {
    regs: [u64; 31],
    sp: u64,
    pc: u64,
    used_altstack: bool,
}

#[derive(Clone, Copy)]
struct AltStackState {
    sp: u64,
    size: usize,
    flags: i32,
}

impl Default for AltStackState {
    fn default() -> Self {
        Self {
            sp: 0,
            size: 0,
            flags: SS_DISABLE,
        }
    }
}

const SS_ONSTACK: i32 = 1;
const SS_DISABLE: i32 = 2;
const MINSIGSTKSZ: usize = 2048;
const SA_ONSTACK: u64 = 0x08000000;

// pid -> (signal -> action)
static SIGACTIONS: Mutex<BTreeMap<u64, BTreeMap<i32, KernelSigAction>>> = Mutex::new(BTreeMap::new());
// pid -> saved pre-handler user context
static SAVED_CONTEXT: Mutex<BTreeMap<u64, SavedUserContext>> = Mutex::new(BTreeMap::new());
// pid -> alternate signal stack state
static ALTSTACKS: Mutex<BTreeMap<u64, AltStackState>> = Mutex::new(BTreeMap::new());

fn valid_signal(sig: i32) -> bool {
    (0..=64).contains(&sig)
}

fn is_fatal_signal(sig: i32) -> bool {
    matches!(sig, 2 | 3 | 6 | 9 | 15)
}

fn queue_signal(pid: u64, sig: i32) {
    if sig <= 0 || sig > 64 {
        return;
    }
    let bit = 1u64 << ((sig - 1) as u32);
    let mut pending = PENDING_SIGNALS.lock();
    let ent = pending.entry(pid).or_insert(0);
    *ent |= bit;
}

fn kill_thread_group(tgid: u64, sig: i32) -> Result<(), i32> {
    let members = {
        let table = crate::sched::process::PROCESS_TABLE.lock();
        table
            .values()
            .filter(|p| p.tgid == tgid)
            .map(|p| p.pid)
            .collect::<alloc::vec::Vec<_>>()
    };
    if members.is_empty() {
        return Err(ESRCH);
    }

    for pid in &members {
        queue_signal(*pid, sig);
    }

    if is_fatal_signal(sig) {
        let code = 128 + sig;
        crate::sched::note_thread_group_exit(tgid, code);
        crate::sched::scheduler::mark_threads_dead(&members);
    }
    Ok(())
}

fn kill_process_group(pgid: u64, sig: i32) -> Result<(), i32> {
    let tgids = {
        let table = crate::sched::process::PROCESS_TABLE.lock();
        let mut groups = alloc::collections::BTreeSet::new();
        for proc in table.values() {
            if proc.pgid == pgid && proc.pid == proc.tgid {
                groups.insert(proc.tgid);
            }
        }
        groups.into_iter().collect::<alloc::vec::Vec<_>>()
    };
    if tgids.is_empty() {
        return Err(ESRCH);
    }
    for tgid in tgids {
        let _ = kill_thread_group(tgid, sig);
    }
    Ok(())
}

fn kill_single_thread(tid: u64, sig: i32) -> Result<(), i32> {
    let exists = {
        let table = crate::sched::process::PROCESS_TABLE.lock();
        table.contains_key(&tid)
    };
    if !exists {
        return Err(ESRCH);
    }

    queue_signal(tid, sig);

    if is_fatal_signal(sig) {
        let code = 128 + sig;
        crate::sched::note_process_exit(tid, code);
        crate::sched::scheduler::mark_threads_dead(&[tid]);
    }
    Ok(())
}

fn sigset_ptr_read(mask_ptr: u64, sigsetsize: usize) -> Result<u64, i32> {
    if mask_ptr == 0 {
        return Ok(0);
    }
    if sigsetsize < 8 {
        return Err(EINVAL);
    }
    let mut raw = [0u8; 8];
    crate::syscall::fs::copy_from_user(mask_ptr, &mut raw).map_err(|_| crate::syscall::EFAULT)?;
    Ok(u64::from_le_bytes(raw))
}

fn sigset_ptr_write(mask_ptr: u64, mask: u64, sigsetsize: usize) -> Result<(), i32> {
    if mask_ptr == 0 {
        return Ok(());
    }
    if sigsetsize < 8 {
        return Err(EINVAL);
    }
    crate::syscall::fs::copy_to_user(mask_ptr, &mask.to_le_bytes()).map_err(|_| crate::syscall::EFAULT)
}

pub fn do_sigaction(signum: i32, act: u64, oldact: u64, sigsetsize: usize) -> SyscallResult {
    if signum <= 0 || signum > 64 {
        return Err(EINVAL);
    }

    let pid = crate::sched::process::current_pid();
    let mut acts = SIGACTIONS.lock();
    let table = acts.entry(pid).or_default();
    let prev = table.get(&signum).copied().unwrap_or_default();

    if oldact != 0 {
        let mut out = [0u8; 32];
        out[0..8].copy_from_slice(&prev.handler.to_le_bytes());
        out[8..16].copy_from_slice(&prev.flags.to_le_bytes());
        out[16..24].copy_from_slice(&prev.restorer.to_le_bytes());
        out[24..32].copy_from_slice(&prev.mask.to_le_bytes());
        crate::syscall::fs::copy_to_user(oldact, &out).map_err(|_| crate::syscall::EFAULT)?;
    }

    if act != 0 {
        let mut inb = [0u8; 32];
        crate::syscall::fs::copy_from_user(act, &mut inb).map_err(|_| crate::syscall::EFAULT)?;
        let new_act = KernelSigAction {
            handler: u64::from_le_bytes(inb[0..8].try_into().unwrap_or([0; 8])),
            flags: u64::from_le_bytes(inb[8..16].try_into().unwrap_or([0; 8])),
            restorer: u64::from_le_bytes(inb[16..24].try_into().unwrap_or([0; 8])),
            mask: u64::from_le_bytes(inb[24..32].try_into().unwrap_or([0; 8])),
        };
        table.insert(signum, new_act);
    }
    Ok(0)
}

pub fn do_sigprocmask(how: i32, set: u64, oldset: u64, sigsetsize: usize) -> SyscallResult {
    const SIG_BLOCK: i32 = 0;
    const SIG_UNBLOCK: i32 = 1;
    const SIG_SETMASK: i32 = 2;

    let pid = crate::sched::process::current_pid();
    let mut blocked = BLOCKED_SIGNALS.lock();
    let cur = *blocked.get(&pid).unwrap_or(&0);

    sigset_ptr_write(oldset, cur, sigsetsize)?;

    if set != 0 {
        let newmask = sigset_ptr_read(set, sigsetsize)?;
        let next = match how {
            SIG_BLOCK => cur | newmask,
            SIG_UNBLOCK => cur & !newmask,
            SIG_SETMASK => newmask,
            _ => return Err(EINVAL),
        };
        blocked.insert(pid, next);
    }
    Ok(0)
}

pub fn replace_current_sigmask(new_mask: u64) -> u64 {
    let pid = crate::sched::process::current_pid();
    let mut blocked = BLOCKED_SIGNALS.lock();
    let old = *blocked.get(&pid).unwrap_or(&0);
    blocked.insert(pid, new_mask);
    old
}

pub fn signalfd_ready(mask: u64) -> bool {
    let pid = crate::sched::process::current_pid();
    let pending = *PENDING_SIGNALS.lock().get(&pid).unwrap_or(&0);
    (pending & mask) != 0
}

pub fn signalfd_consume(mask: u64) -> Option<i32> {
    let pid = crate::sched::process::current_pid();
    let mut pending = PENDING_SIGNALS.lock();
    let bits = *pending.get(&pid).unwrap_or(&0);
    let masked = bits & mask;
    if masked == 0 {
        return None;
    }
    for s in 1..=64 {
        let bit = 1u64 << ((s - 1) as u32);
        if (masked & bit) != 0 {
            if let Some(ent) = pending.get_mut(&pid) {
                *ent &= !bit;
            }
            return Some(s);
        }
    }
    None
}

pub fn do_sigaltstack(ss: u64, old_ss: u64) -> SyscallResult {
    let pid = crate::sched::process::current_pid();
    let mut stacks = ALTSTACKS.lock();
    let current = stacks.get(&pid).copied().unwrap_or_default();

    if old_ss != 0 {
        let mut out = [0u8; 24];
        out[0..8].copy_from_slice(&current.sp.to_le_bytes());
        out[8..12].copy_from_slice(&current.flags.to_le_bytes());
        out[16..24].copy_from_slice(&(current.size as u64).to_le_bytes());
        crate::syscall::fs::copy_to_user(old_ss, &out).map_err(|_| crate::syscall::EFAULT)?;
    }

    if ss != 0 {
        let mut inb = [0u8; 24];
        crate::syscall::fs::copy_from_user(ss, &mut inb).map_err(|_| crate::syscall::EFAULT)?;
        let sp = u64::from_le_bytes(inb[0..8].try_into().unwrap_or([0; 8]));
        let flags = i32::from_le_bytes(inb[8..12].try_into().unwrap_or([0; 4]));
        let size = u64::from_le_bytes(inb[16..24].try_into().unwrap_or([0; 8])) as usize;

        if (flags & !(SS_DISABLE)) != 0 {
            return Err(EINVAL);
        }
        if (current.flags & SS_ONSTACK) != 0 {
            return Err(EINVAL);
        }
        let next = if (flags & SS_DISABLE) != 0 {
            AltStackState {
                sp: 0,
                size: 0,
                flags: SS_DISABLE,
            }
        } else {
            if sp == 0 || size < MINSIGSTKSZ {
                return Err(EINVAL);
            }
            AltStackState { sp, size, flags: 0 }
        };
        stacks.insert(pid, next);
    }
    Ok(0)
}

pub fn do_sigreturn(frame: &mut impl CpuContext) -> SyscallResult {
    let pid = crate::sched::process::current_pid();
    let saved = {
        let mut map = SAVED_CONTEXT.lock();
        map.remove(&pid).ok_or(EINVAL)?
    };
    for (i, v) in saved.regs.iter().enumerate() {
        frame.set_reg(i, *v);
    }
    frame.set_sp(saved.sp);
    frame.set_pc(saved.pc);
    if saved.used_altstack {
        if let Some(st) = ALTSTACKS.lock().get_mut(&pid) {
            st.flags &= !SS_ONSTACK;
        }
    }
    IN_SIGNAL_HANDLER.lock().remove(&pid);
    Ok(saved.regs[0] as usize)
}

pub fn do_kill(pid: i32, sig: i32) -> SyscallResult {
    if !valid_signal(sig) {
        return Err(EINVAL);
    }
    if sig == 0 {
        // Existence check only.
        if pid > 0 {
            let exists = {
                let table = crate::sched::process::PROCESS_TABLE.lock();
                table.values().any(|p| p.tgid == pid as u64)
            };
            return if exists { Ok(0) } else { Err(ESRCH) };
        } else if pid == 0 {
            let caller = crate::sched::process::current_pid();
            let pgrp = {
                let table = crate::sched::process::PROCESS_TABLE.lock();
                table.get(&caller).map(|p| p.pgid).unwrap_or(caller)
            };
            return kill_process_group(pgrp, sig).map(|_| 0usize);
        } else if pid < -1 {
            return kill_process_group((-pid) as u64, sig).map(|_| 0usize);
        }
        return Ok(0);
    }

    if pid > 0 {
        kill_thread_group(pid as u64, sig)?;
    } else if pid == 0 {
        let caller = crate::sched::process::current_pid();
        let pgrp = {
            let table = crate::sched::process::PROCESS_TABLE.lock();
            table.get(&caller).map(|p| p.pgid).unwrap_or(caller)
        };
        deliver_signal_to_pgrp(pgrp as i32, sig);
    } else if pid == -1 {
        // Broadcast to all process groups except kernel idle.
        let tgids = {
            let table = crate::sched::process::PROCESS_TABLE.lock();
            let mut set = alloc::collections::BTreeSet::new();
            for p in table.values() {
                if p.tgid != 0 {
                    set.insert(p.tgid);
                }
            }
            set.into_iter().collect::<alloc::vec::Vec<_>>()
        };
        for tgid in tgids {
            let _ = kill_thread_group(tgid, sig);
        }
    } else {
        // pid < -1 => process group -pid
        deliver_signal_to_pgrp((-pid) as i32, sig);
    }
    Ok(0)
}

pub fn do_tgkill(tgid: i32, tid: i32, sig: i32) -> SyscallResult {
    if !valid_signal(sig) || tgid <= 0 || tid <= 0 {
        return Err(EINVAL);
    }
    let (exists, is_leader) = {
        let table = crate::sched::process::PROCESS_TABLE.lock();
        if let Some(proc) = table.get(&(tid as u64)) {
            (proc.tgid == tgid as u64, proc.pid == proc.tgid)
        } else {
            (false, false)
        }
    };
    if !exists {
        return Err(ESRCH);
    }
    if sig == 0 {
        return Ok(0);
    }
    if is_leader {
        kill_thread_group(tgid as u64, sig)?;
    } else {
        kill_single_thread(tid as u64, sig)?;
    }
    Ok(0)
}

/// Deliver a signal to all processes in a process group.
pub fn deliver_signal_to_pgrp(pgrp: i32, signum: i32) {
    if pgrp <= 0 || !valid_signal(signum) {
        return;
    }
    let _ = kill_process_group(pgrp as u64, signum);
}

pub fn dispatch_pending(frame: &mut impl CpuContext) {
    let pid = crate::sched::process::current_pid();
    if IN_SIGNAL_HANDLER.lock().contains(&pid) {
        return;
    }

    let blocked = *BLOCKED_SIGNALS.lock().get(&pid).unwrap_or(&0);
    let pending_bits = *PENDING_SIGNALS.lock().get(&pid).unwrap_or(&0);
    let deliverable = pending_bits & !blocked;
    if deliverable == 0 {
        return;
    }

    let mut sig = 0i32;
    for s in 1..=64 {
        let bit = 1u64 << ((s - 1) as u32);
        if deliverable & bit != 0 {
            sig = s;
            break;
        }
    }
    if sig == 0 {
        return;
    }

    {
        let mut pending = PENDING_SIGNALS.lock();
        if let Some(bits) = pending.get_mut(&pid) {
            let bit = 1u64 << ((sig - 1) as u32);
            *bits &= !bit;
        }
    }

    let action = {
        let acts = SIGACTIONS.lock();
        acts.get(&pid)
            .and_then(|m| m.get(&sig).copied())
            .unwrap_or_default()
    };

    // SIG_IGN
    if action.handler == 1 {
        return;
    }

    // Default action for fatal signals.
    if action.handler == 0 {
        if is_fatal_signal(sig) {
            let code = 128 + sig;
            crate::sched::note_process_exit(pid, code);
            crate::sched::scheduler::mark_threads_dead(&[pid]);
            crate::sched::scheduler::schedule();
        }
        return;
    }

    let mut saved = SavedUserContext {
        regs: [0u64; 31],
        sp: frame.sp(),
        pc: frame.pc(),
        used_altstack: false,
    };
    for i in 0..31 {
        saved.regs[i] = frame.reg(i);
    }
    if (action.flags & SA_ONSTACK) != 0 {
        let mut stacks = ALTSTACKS.lock();
        let st = stacks.entry(pid).or_default();
        if (st.flags & SS_DISABLE) == 0 && st.size > 0 {
            saved.used_altstack = true;
            st.flags |= SS_ONSTACK;
            frame.set_sp(st.sp.saturating_add(st.size as u64));
        }
    }
    SAVED_CONTEXT.lock().insert(pid, saved);
    IN_SIGNAL_HANDLER.lock().insert(pid);

    // Enter user handler: x0 = signum, LR = restorer (if provided).
    frame.set_reg(0, sig as u64);
    frame.set_reg(1, 0);
    frame.set_reg(2, 0);
    if action.restorer != 0 {
        frame.set_reg(30, action.restorer);
    }
    frame.set_pc(action.handler);
}
