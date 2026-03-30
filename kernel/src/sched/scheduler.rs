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

    let mut sched = SCHEDULER.lock();
    if !sched.run_queues.is_empty() {
        let min_cpu = (0..sched.num_cpus)
            .min_by_key(|&i| sched.run_queues[i].len())
            .unwrap_or(0);
        sched.run_queues[min_cpu].push_back(thread);
    }

    log::debug!("Spawned kernel thread '{name}' (PID {pid})");
    pid
}

pub fn schedule() {
    let cpu_id = current_cpu_id();

    let mut sched = SCHEDULER.lock();
    if !sched.initialized || cpu_id >= sched.num_cpus {
        return;
    }

    // Check if there's anything to switch to
    let next = sched.run_queues[cpu_id].pop_front();
    if next.is_none() {
        // Try work stealing from other CPUs
        let stolen = try_steal(&mut sched, cpu_id);
        if stolen.is_none() {
            return;
        }
        do_switch(&mut sched, cpu_id, stolen.unwrap());
        return;
    }

    do_switch(&mut sched, cpu_id, next.unwrap());
}

fn try_steal(sched: &mut Scheduler, cpu_id: usize) -> Option<Thread> {
    for i in 0..sched.num_cpus {
        if i == cpu_id {
            continue;
        }
        if sched.run_queues[i].len() > 1 {
            return sched.run_queues[i].pop_back();
        }
    }
    None
}

fn do_switch(sched: &mut Scheduler, cpu_id: usize, mut next: Thread) {
    let mut current = sched.current[cpu_id].take().unwrap();

    if current.pid == 0 && !next.is_user {
        // Idle -> kernel thread: just replace
        next.state = ThreadState::Running;
        sched.current[cpu_id] = Some(next);
        return;
    }

    // For user threads on first run, set up x19-x22 for the trampoline
    if next.is_user && next.state == ThreadState::Ready {
        next.context.x19 = next.context.elr;     // entry point
        next.context.x20 = next.context.spsr;    // SPSR (EL0t)
        next.context.x21 = next.context.sp_el0;  // user stack
        next.context.x22 = next.context.ttbr0;   // page table
    }

    if current.state == ThreadState::Running {
        current.state = ThreadState::Ready;
        if current.pid != 0 {
            sched.run_queues[cpu_id].push_back(current);
        }
    }

    next.state = ThreadState::Running;

    // For user threads, switch TTBR0
    if next.is_user {
        unsafe {
            core::arch::asm!(
                "msr TTBR0_EL1, {}",
                "isb",
                in(reg) next.context.ttbr0,
            );
        }
    }

    sched.current[cpu_id] = Some(next);

    // Real context switch would happen here via context_switch()
    // For now, the trampoline handles first-time user entry
}

fn current_cpu_id() -> usize {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr) };
    (mpidr & 0xFF) as usize
}

// Context switch assembly
global_asm!(
    ".global context_switch",
    "context_switch:",
    // x0 = old Context*, x1 = new Context*
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
);

unsafe extern "C" {
    pub fn context_switch(old: *mut Context, new: *const Context);
}
