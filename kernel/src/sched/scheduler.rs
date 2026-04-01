use super::process::{Thread, ThreadState, Pid, Context};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use spin::Mutex;
use core::arch::global_asm;

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

pub fn spawn_kernel_thread(name: &str, entry: fn()) -> Pid {
    let thread = Thread::new_kernel(name, entry);
    let pid = thread.pid;
    let target_cpu;
    {
        let mut sched = SCHEDULER.lock();
        if sched.run_queues.is_empty() {
            return pid;
        }
        // Phase 1: all tasks on BSP (CPU 0) only
        target_cpu = 0;
        sched.run_queues[target_cpu].push_back(thread);
    }
    // TODO Phase 3: send_resched_ipi(target_cpu) when target != current
    log::debug!("Spawned kernel thread '{name}' (PID {pid}) on CPU {target_cpu}");
    pid
}

/// Schedule on the current CPU. Called from timer_tick and SGI handler.
pub fn schedule() {
    let cpu_id = current_cpu_id();

    // Determine what to switch under the lock, perform the actual switch outside.
    let switch_info: Option<(*mut Context, *const Context, bool, Option<u64>)> = {
        let mut sched = SCHEDULER.lock();
        if !sched.initialized || cpu_id >= sched.num_cpus {
            return;
        }

        // Find next thread for this CPU (Phase 1: no work stealing)
        let next = sched.run_queues[cpu_id].pop_front();

        let mut next = match next {
            Some(t) => t,
            None => return, // Nothing to run, stay idle
        };

        let mut current = match sched.current[cpu_id].take() {
            Some(t) => t,
            None => return,
        };

        // For user threads on first run, set trampoline registers
        if next.is_user && next.state == ThreadState::Ready {
            next.context.x19 = next.context.elr;     // entry point
            next.context.x20 = next.context.spsr;    // SPSR (EL0t = 0)
            next.context.x21 = next.context.sp_el0;  // user stack
            next.context.x22 = next.context.ttbr0;   // page table
        }

        // Extract TTBR0 to switch OUTSIDE the lock (avoids tlbi+dsb stall under spinlock)
        let ttbr0 = if next.is_user { Some(next.context.ttbr0) } else { None };

        if current.pid == 0 {
            // Idle → task: just replace, no context to save
            next.state = ThreadState::Running;
            sched.current[cpu_id] = Some(next);
            let ctx_ptr = &sched.current[cpu_id].as_ref().unwrap().context as *const Context;
            Some((core::ptr::null_mut::<Context>(), ctx_ptr, true, ttbr0))
        } else {
            if current.state == ThreadState::Running {
                current.state = ThreadState::Ready;
            }
            next.state = ThreadState::Running;

            sched.run_queues[cpu_id].push_back(current);
            sched.current[cpu_id] = Some(next);

            let current_ctx = &mut sched.run_queues[cpu_id].back_mut().unwrap().context as *mut Context;
            let next_ctx = &sched.current[cpu_id].as_ref().unwrap().context as *const Context;
            Some((current_ctx, next_ctx, false, ttbr0))
        }
    };
    // Lock is now released.

    if let Some((old_ctx, new_ctx, is_idle_to_user, ttbr0)) = switch_info {
        // Switch TTBR0 outside the lock — TLB broadcast won't stall other cores' lock acquisition
        if let Some(t) = ttbr0 {
            unsafe {
                core::arch::asm!(
                    "msr TTBR0_EL1, {0}",
                    "isb",
                    in(reg) t,
                );
            }
        }

        if is_idle_to_user {
            unsafe { switch_to_new(new_ctx); }
        } else if !old_ctx.is_null() {
            unsafe { context_switch(old_ctx, new_ctx); }
        }
    }
}

fn current_cpu_id() -> usize {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr) };
    (mpidr & 0xFF) as usize
}

/// Send a reschedule IPI to wake another CPU from idle.
pub fn send_resched_ipi(target_cpu: usize) {
    #[cfg(target_arch = "aarch64")]
    crate::arch::aarch64::gic::send_sgi(target_cpu, crate::arch::aarch64::gic::SGI_RESCHEDULE);
}

// Context switch: save old callee-saved regs, restore new ones, ret
global_asm!(
    ".global context_switch",
    "context_switch:",
    "stp x19, x20, [x0, #0]",
    "stp x21, x22, [x0, #16]",
    "stp x23, x24, [x0, #32]",
    "stp x25, x26, [x0, #48]",
    "stp x27, x28, [x0, #64]",
    "stp x29, x30, [x0, #80]",
    "mov x2, sp",
    "str x2, [x0, #96]",

    "ldp x19, x20, [x1, #0]",
    "ldp x21, x22, [x1, #16]",
    "ldp x23, x24, [x1, #32]",
    "ldp x25, x26, [x1, #48]",
    "ldp x27, x28, [x1, #64]",
    "ldp x29, x30, [x1, #80]",
    "ldr x2, [x1, #96]",
    "mov sp, x2",
    "ret",

    // switch_to_new: just restore new context registers and jump
    ".global switch_to_new",
    "switch_to_new:",
    "mov x1, x0",   // x0 = new_ctx, move to x1 for consistency
    "ldp x19, x20, [x1, #0]",
    "ldp x21, x22, [x1, #16]",
    "ldp x23, x24, [x1, #32]",
    "ldp x25, x26, [x1, #48]",
    "ldp x27, x28, [x1, #64]",
    "ldp x29, x30, [x1, #80]",
    "ldr x2, [x1, #96]",
    "mov sp, x2",
    "ret",
);

unsafe extern "C" {
    pub fn context_switch(old: *mut Context, new: *const Context);
    pub fn switch_to_new(new: *const Context);
}
