mod process;
mod scheduler;
pub mod cpu;

pub use process::{Process, Thread, Pid, ThreadState};
pub use scheduler::SCHEDULER;

use crate::syscall::{SyscallResult, ENOSYS, ECHILD, ENOMEM};

/// Called on every timer tick from the interrupt handler
pub fn timer_tick() {
    scheduler::schedule();
}

/// Initialize the scheduler with an idle thread for each CPU
pub fn init(num_cpus: usize) {
    scheduler::init(num_cpus);
}

/// Spawn a new kernel thread
pub fn spawn_kernel_thread(name: &str, entry: fn()) -> Pid {
    scheduler::spawn_kernel_thread(name, entry)
}

/// clone syscall implementation
pub fn do_clone(flags: u64, stack: u64, ptid: u64, tls: u64, ctid: u64) -> SyscallResult {
    // Stub: will implement proper fork/clone later
    Ok(0) // Return 0 = child
}

/// execve syscall
pub fn do_execve(pathname: u64, argv: u64, envp: u64) -> SyscallResult {
    // Will load ELF and replace current process
    Err(ENOSYS)
}

/// wait4 syscall
pub fn do_wait4(pid: i32, wstatus: u64, options: u32, rusage: u64) -> SyscallResult {
    // No children to wait for yet
    Err(ECHILD)
}
