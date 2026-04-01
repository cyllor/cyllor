pub mod process;
pub mod scheduler;
pub mod cpu;
pub mod elf;

pub use process::{Process, Thread, Pid, ThreadState, Context};
pub use scheduler::SCHEDULER;

use crate::syscall::{SyscallResult, ENOSYS, ECHILD, ENOMEM, ENOENT};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

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

/// Create and run a user process from an ELF binary in the filesystem
pub fn spawn_user_process(path: &str, argv: &[&[u8]], envp: &[&[u8]]) -> Result<Pid, &'static str> {
    // Read the ELF file
    let node = crate::fs::vfs::resolve_path(path).map_err(|_| "File not found")?;
    let data = {
        let n = node.lock();
        n.data.clone()
    };

    if data.is_empty() {
        return Err("Empty file");
    }

    // Create address space
    let aspace = crate::arch::aarch64::paging::AddressSpace::new()
        .ok_or("Failed to allocate address space")?;

    // Load ELF
    let result = elf::load_elf(&data, &aspace)?;

    // Set up user stack
    let sp = elf::setup_user_stack(&aspace, result.stack_top, argv, envp, &result)?;

    // Initialize brk from ELF brk_start
    crate::mm::mmap::set_brk_base(result.brk_start as usize);

    // (debug address checks removed)

    // Create thread with userspace context
    let pid = process::alloc_pid();
    let mut thread = Thread::new_user(
        path,
        pid,
        result.entry,
        sp,
        aspace,
    );

    // Add to scheduler
    let mut sched = scheduler::SCHEDULER.lock();
    if !sched.run_queues.is_empty() {
        let min_cpu = (0..sched.num_cpus)
            .min_by_key(|&i| sched.run_queues[i].len())
            .unwrap_or(0);
        sched.run_queues[min_cpu].push_back(thread);
    }

    log::info!("Spawned user process '{}' (PID {pid}) entry=0x{:x}", path, result.entry);
    Ok(pid)
}

/// clone syscall implementation
pub fn do_clone(flags: u64, stack: u64, ptid: u64, tls: u64, ctid: u64) -> SyscallResult {
    // TODO: proper fork/clone
    Ok(0)
}

/// execve syscall
pub fn do_execve(pathname: u64, argv: u64, envp: u64) -> SyscallResult {
    // TODO: replace current process image
    Err(ENOSYS)
}

/// wait4 syscall
pub fn do_wait4(pid: i32, wstatus: u64, options: u32, rusage: u64) -> SyscallResult {
    Err(ECHILD)
}
