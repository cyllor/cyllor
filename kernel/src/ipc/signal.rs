use crate::arch::TrapFrame;
use crate::syscall::{SyscallResult, EINVAL, ENOSYS};

pub fn do_sigaction(signum: i32, act: u64, oldact: u64, sigsetsize: usize) -> SyscallResult {
    // Stub: accept signal handler registrations silently
    if oldact != 0 {
        unsafe { core::ptr::write_bytes(oldact as *mut u8, 0, 152); } // sizeof(struct sigaction)
    }
    Ok(0)
}

pub fn do_sigprocmask(how: i32, set: u64, oldset: u64, sigsetsize: usize) -> SyscallResult {
    if oldset != 0 {
        unsafe { core::ptr::write_bytes(oldset as *mut u8, 0, 8); }
    }
    Ok(0)
}

pub fn do_sigreturn(frame: &mut TrapFrame) -> SyscallResult {
    // TODO: restore signal frame
    Ok(0)
}

pub fn do_kill(pid: i32, sig: i32) -> SyscallResult {
    // Stub
    Ok(0)
}

pub fn do_tgkill(tgid: i32, tid: i32, sig: i32) -> SyscallResult {
    Ok(0)
}

/// Deliver a signal to all processes in a process group (stub).
/// In a full implementation this would walk the process table.
pub fn deliver_signal_to_pgrp(_pgrp: i32, _signum: i32) {
    // TODO: iterate process table and deliver signal to matching pgrp
}
