use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

pub type Pid = u64;

static NEXT_PID: AtomicU64 = AtomicU64::new(1);

pub fn alloc_pid() -> Pid {
    NEXT_PID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Ready,
    Running,
    Blocked,
    Dead,
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct Context {
    // Callee-saved registers: x19-x30, sp
    pub x19: u64,
    pub x20: u64,
    pub x21: u64,
    pub x22: u64,
    pub x23: u64,
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    pub x28: u64,
    pub x29: u64, // frame pointer
    pub x30: u64, // link register (return address)
    pub sp: u64,
    pub elr: u64,   // for userspace: return address
    pub spsr: u64,  // for userspace: saved PSTATE
    pub sp_el0: u64, // userspace stack pointer
}

impl Context {
    pub const fn zero() -> Self {
        Self {
            x19: 0, x20: 0, x21: 0, x22: 0, x23: 0, x24: 0,
            x25: 0, x26: 0, x27: 0, x28: 0, x29: 0, x30: 0,
            sp: 0, elr: 0, spsr: 0, sp_el0: 0,
        }
    }
}

pub struct Thread {
    pub pid: Pid,
    pub name: String,
    pub state: ThreadState,
    pub context: Context,
    pub kernel_stack: Vec<u8>,
    pub vruntime: u64, // for CFS scheduling
    pub is_user: bool,
}

const KERNEL_STACK_SIZE: usize = 64 * 1024; // 64 KiB

impl Thread {
    pub fn new_kernel(name: &str, entry: fn()) -> Self {
        let pid = alloc_pid();
        let mut stack = Vec::with_capacity(KERNEL_STACK_SIZE);
        stack.resize(KERNEL_STACK_SIZE, 0u8);

        let stack_top = stack.as_ptr() as u64 + KERNEL_STACK_SIZE as u64;

        let mut ctx = Context::zero();
        ctx.x30 = entry as u64; // return address = entry point
        ctx.sp = stack_top;

        Thread {
            pid,
            name: String::from(name),
            state: ThreadState::Ready,
            context: ctx,
            kernel_stack: stack,
            vruntime: 0,
            is_user: false,
        }
    }

    pub fn new_idle(cpu_id: usize) -> Self {
        Thread {
            pid: 0,
            name: alloc::format!("idle-{cpu_id}"),
            state: ThreadState::Running,
            context: Context::zero(),
            kernel_stack: Vec::new(),
            vruntime: u64::MAX, // idle always has worst priority
            is_user: false,
        }
    }
}

pub struct Process {
    pub pid: Pid,
    pub name: String,
    pub threads: Vec<Pid>,
    // Phase 4+: address space, file descriptors, etc.
}
