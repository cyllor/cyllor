use super::process::{Thread, ThreadState, Pid, Context};
use super::cpu;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use spin::Mutex;

pub static SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());

pub struct Scheduler {
    pub run_queues: Vec<VecDeque<Thread>>,
    pub current: Vec<Option<Thread>>,
    pub num_cpus: usize,
    pub initialized: bool,
}

impl Scheduler {
    pub const fn new() -> Self {
        Self {
            run_queues: Vec::new(),
            current: Vec::new(),
            num_cpus: 0,
            initialized: false,
        }
    }
}

pub fn init(num_cpus: usize) {
    let mut sched = SCHEDULER.lock();
    sched.num_cpus = num_cpus;
    for i in 0..num_cpus {
        sched.run_queues.push(VecDeque::new());
        sched.current.push(Some(Thread::new_idle(i)));
    }
    sched.initialized = true;
    log::info!("Scheduler initialized for {num_cpus} CPUs");
}

/// Return the index of the CPU with the fewest Ready threads.
pub fn least_loaded_cpu(sched: &Scheduler) -> usize {
    (0..sched.num_cpus)
        .min_by_key(|&c| {
            sched.run_queues[c]
                .iter()
                .filter(|t| t.state == ThreadState::Ready)
                .count()
        })
        .unwrap_or(0)
}

pub fn spawn_kernel_thread(name: &str, entry: fn()) -> Pid {
    let thread = Thread::new_kernel(name, entry);
    let pid = thread.pid;
    let target_cpu;
    {
        let mut sched = SCHEDULER.lock();
        if sched.run_queues.is_empty() { return pid; }
        target_cpu = least_loaded_cpu(&sched);
        sched.run_queues[target_cpu].push_back(thread);
    }
    if target_cpu != cpu::current_cpu_id() {
        send_resched_ipi(target_cpu);
    }
    log::debug!("Spawned kernel thread '{name}' (PID {pid}) on CPU {target_cpu}");
    pid
}

/// Schedule on the current CPU.
/// Called from `timer_tick()` (timer interrupt) and SGI handler (resched IPI).
pub fn schedule() {
    let cpu_id = cpu::current_cpu_id();

    let switch_info: Option<(*mut Context, *const Context, bool, Option<u64>)> = {
        let mut sched = SCHEDULER.lock();
        if !sched.initialized || cpu_id >= sched.num_cpus {
            return;
        }

        // Increment the running thread's vruntime (time-slice accounting).
        if let Some(curr) = sched.current[cpu_id].as_mut() {
            if curr.pid != 0 {
                curr.vruntime = curr.vruntime.saturating_add(1);
            }
        }

        // ── Pick next thread ────────────────────────────────────────────────
        // 1. Local queue: CFS-like — choose Ready thread with minimum vruntime.
        let local_next = {
            let idx = sched.run_queues[cpu_id]
                .iter()
                .enumerate()
                .filter(|(_, t)| t.state == ThreadState::Ready)
                .min_by_key(|(_, t)| t.vruntime)
                .map(|(i, _)| i);
            idx.and_then(|i| sched.run_queues[cpu_id].remove(i))
        };

        // 2. Work-stealing: if local queue has nothing runnable, steal one
        //    thread from the busiest remote CPU (only if victim has > 1 Ready).
        let next = local_next.or_else(|| {
            let num_cpus = sched.num_cpus;
            let victim = (0..num_cpus)
                .filter(|&c| c != cpu_id)
                .max_by_key(|&c| {
                    sched.run_queues[c]
                        .iter()
                        .filter(|t| t.state == ThreadState::Ready)
                        .count()
                });
            victim.and_then(|v| {
                let ready = sched.run_queues[v]
                    .iter()
                    .filter(|t| t.state == ThreadState::Ready)
                    .count();
                if ready > 1 {
                    // Steal the last Ready thread (back of victim's queue).
                    let steal_idx = sched.run_queues[v]
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(_, t)| t.state == ThreadState::Ready)
                        .map(|(i, _)| i);
                    steal_idx.and_then(|i| sched.run_queues[v].remove(i))
                } else {
                    None
                }
            })
        });

        // Nothing runnable — stay on the current thread.
        let mut next = match next {
            Some(t) => t,
            None => return,
        };

        // Take the currently running thread out of current[].
        let mut current = match sched.current[cpu_id].take() {
            Some(t) => t,
            None => return,
        };

        // First-run setup for user threads: load entry/spsr/sp_el0/ttbr0 into
        // x19-x22 so return_to_user_trampoline can ERET to EL0.
        if next.is_user && next.first_run {
            next.context.x19 = next.context.elr;
            next.context.x20 = next.context.spsr;
            next.context.x21 = next.context.sp_el0;
            next.context.x22 = next.context.ttbr0;
            next.first_run = false;
        }

        let ttbr0 = if next.is_user { Some(next.context.ttbr0) } else { None };
        let next_pid = next.pid;
        next.state = ThreadState::Running;

        if current.pid == 0 {
            // Idle → new thread: just restore new context, nothing to save.
            sched.current[cpu_id] = Some(next);
            cpu::set_current_pid(cpu_id, next_pid);
            let ctx = &sched.current[cpu_id].as_ref().unwrap().context as *const Context;
            Some((core::ptr::null_mut(), ctx, true, ttbr0))
        } else {
            // Re-queue the previous thread.
            // If it was Running, demote to Ready.  If Blocked, keep Blocked so
            // wait::tick() can find and wake it later.
            if current.state == ThreadState::Running {
                current.state = ThreadState::Ready;
            }
            sched.run_queues[cpu_id].push_back(current);
            sched.current[cpu_id] = Some(next);
            cpu::set_current_pid(cpu_id, next_pid);

            let old_ctx =
                &mut sched.run_queues[cpu_id].back_mut().unwrap().context as *mut Context;
            let new_ctx =
                &sched.current[cpu_id].as_ref().unwrap().context as *const Context;
            Some((old_ctx, new_ctx, false, ttbr0))
        }
    }; // Scheduler lock released.

    if let Some((old_ctx, new_ctx, is_idle_to_new, ttbr0)) = switch_info {
        // Switch the user page table outside the lock to avoid TLB broadcast
        // stalling other CPUs' lock acquisitions.
        if let Some(t) = ttbr0 {
            crate::arch::activate_user_page_table(t);
        }
        if is_idle_to_new {
            unsafe { crate::arch::aarch64::context::switch_to_new_asm(new_ctx); }
        } else if !old_ctx.is_null() {
            unsafe { crate::arch::aarch64::context::context_switch_asm(old_ctx, new_ctx); }
        }
    }
}

/// Mark the current thread Blocked until `wake_tick`, then yield.
/// Returns once the thread has been woken and rescheduled.
pub fn block_current_until(wake_tick: u64) {
    let cpu_id = cpu::current_cpu_id();
    let pid = cpu::get_current_pid();

    {
        let mut sched = SCHEDULER.lock();
        if let Some(curr) = sched.current[cpu_id].as_mut() {
            curr.state = ThreadState::Blocked;
        }
    }

    // Register wakeup outside the scheduler lock (wait has its own lock).
    super::wait::register_wakeup(pid, wake_tick);

    // Yield — schedule() re-queues us as Blocked; we resume when woken.
    schedule();
}

/// Send a reschedule SGI to wake an idle CPU.
pub fn send_resched_ipi(target_cpu: usize) {
    #[cfg(target_arch = "aarch64")]
    crate::arch::aarch64::gic::send_sgi(
        target_cpu,
        crate::arch::aarch64::gic::SGI_RESCHEDULE,
    );
}

