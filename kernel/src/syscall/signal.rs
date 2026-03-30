#[cfg(target_arch = "aarch64")]
use crate::arch::aarch64::exceptions::TrapFrame;
use super::{SyscallResult, ENOSYS, EINVAL};

pub fn sys_rt_sigaction(signum: i32, act: u64, oldact: u64, sigsetsize: u64) -> SyscallResult {
    crate::ipc::signal::do_sigaction(signum, act, oldact, sigsetsize as usize)
}

pub fn sys_rt_sigprocmask(how: i32, set: u64, oldset: u64, sigsetsize: u64) -> SyscallResult {
    crate::ipc::signal::do_sigprocmask(how, set, oldset, sigsetsize as usize)
}

pub fn sys_rt_sigreturn(frame: &mut TrapFrame) -> SyscallResult {
    crate::ipc::signal::do_sigreturn(frame)
}

pub fn sys_kill(pid: i32, sig: i32) -> SyscallResult {
    crate::ipc::signal::do_kill(pid, sig)
}

pub fn sys_tgkill(tgid: i32, tid: i32, sig: i32) -> SyscallResult {
    crate::ipc::signal::do_tgkill(tgid, tid, sig)
}
