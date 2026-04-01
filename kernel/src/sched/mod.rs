pub mod cpu;
pub mod elf;
pub mod process;
pub mod scheduler;
pub mod wait;

pub use process::{Context, Pid, Process, Thread, ThreadState};
pub use scheduler::SCHEDULER;

use crate::syscall::{SyscallResult, ECHILD, ENOSYS};

/// Called on every timer interrupt.
/// Advances the sleep clock, then schedules on the current CPU.
pub fn timer_tick() {
    wait::tick();
    scheduler::schedule();
}

/// Initialize the scheduler with one idle thread per CPU.
pub fn init(num_cpus: usize) {
    scheduler::init(num_cpus);
}

/// Spawn a kernel thread, distributing it to the least-loaded CPU.
pub fn spawn_kernel_thread(name: &str, entry: fn()) -> Pid {
    scheduler::spawn_kernel_thread(name, entry)
}

/// Load and start a user process from the VFS, distributing it to the
/// least-loaded CPU and sending a resched IPI if the target is remote.
pub fn spawn_user_process(
    path: &str,
    argv: &[&[u8]],
    envp: &[&[u8]],
) -> Result<Pid, &'static str> {
    let node = crate::fs::vfs::resolve_path(path).map_err(|_| "file not found")?;
    let data = node.lock().data.clone();
    if data.is_empty() {
        return Err("empty file");
    }

    let aspace = crate::arch::AddressSpace::new()
        .ok_or("failed to allocate address space")?;

    let result = elf::load_elf(&data, &aspace)?;
    let sp = elf::setup_user_stack(&aspace, result.stack_top, argv, envp, &result)?;

    crate::mm::mmap::set_brk_base(result.brk_start as usize);

    let pid = process::alloc_pid();
    let thread = Thread::new_user(path, pid, result.entry, sp, aspace);

    let target_cpu;
    {
        let mut sched = scheduler::SCHEDULER.lock();
        if sched.run_queues.is_empty() {
            return Err("scheduler not initialized");
        }
        target_cpu = scheduler::least_loaded_cpu(&sched);
        sched.run_queues[target_cpu].push_back(thread);
    }

    if target_cpu != cpu::current_cpu_id() {
        scheduler::send_resched_ipi(target_cpu);
    }

    log::info!(
        "Spawned user process '{}' (PID {pid}) entry=0x{:x} on CPU {target_cpu}",
        path,
        result.entry
    );
    Ok(pid)
}

// ── Phase 6 stubs ────────────────────────────────────────────────────────────

/// clone syscall — Phase 6.
pub fn do_clone(_flags: u64, _stack: u64, _ptid: u64, _tls: u64, _ctid: u64) -> SyscallResult {
    Ok(0)
}

/// execve syscall — Phase 6.
pub fn do_execve(_pathname: u64, _argv: u64, _envp: u64) -> SyscallResult {
    Err(ENOSYS)
}

/// wait4 syscall — Phase 6.
pub fn do_wait4(_pid: i32, _wstatus: u64, _options: u32, _rusage: u64) -> SyscallResult {
    Err(ECHILD)
}
