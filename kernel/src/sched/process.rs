use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::AddressSpace;

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
    // Callee-saved registers saved/restored by context_switch assembly.
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
    pub x30: u64, // link register (return address / trampoline)
    pub sp: u64,
    // User-mode fields (not saved by context_switch; used only for first-run setup).
    pub elr: u64,    // EL0 entry point
    pub spsr: u64,   // SPSR_EL1 value for ERET to EL0
    pub sp_el0: u64, // user stack pointer
    pub ttbr0: u64,  // user page-table root
}

impl Context {
    pub const fn zero() -> Self {
        Self {
            x19: 0, x20: 0, x21: 0, x22: 0, x23: 0, x24: 0,
            x25: 0, x26: 0, x27: 0, x28: 0, x29: 0, x30: 0,
            sp: 0, elr: 0, spsr: 0, sp_el0: 0, ttbr0: 0,
        }
    }
}

const KERNEL_STACK_SIZE: usize = 64 * 1024; // 64 KiB

pub struct Thread {
    pub pid: Pid,
    pub name: String,
    pub state: ThreadState,
    pub context: Context,
    pub kernel_stack: Vec<u8>,
    pub vruntime: u64,
    pub is_user: bool,
    /// True until the thread has been context-switched to for the first time.
    /// The scheduler uses this to load ELR/SPSR/SP_EL0 into x19-x22 for the
    /// return_to_user_trampoline before clearing the flag.
    pub first_run: bool,
    pub address_space: Option<AddressSpace>,
}

impl Thread {
    pub fn new_kernel(name: &str, entry: fn()) -> Self {
        let pid = alloc_pid();
        let mut stack = Vec::with_capacity(KERNEL_STACK_SIZE);
        stack.resize(KERNEL_STACK_SIZE, 0u8);
        let stack_top = stack.as_ptr() as u64 + KERNEL_STACK_SIZE as u64;

        let mut ctx = Context::zero();
        ctx.x30 = entry as u64;
        ctx.sp = stack_top;

        Thread {
            pid,
            name: String::from(name),
            state: ThreadState::Ready,
            context: ctx,
            kernel_stack: stack,
            vruntime: 0,
            is_user: false,
            first_run: false,
            address_space: None,
        }
    }

    pub fn new_idle(cpu_id: usize) -> Self {
        Thread {
            pid: 0,
            name: alloc::format!("idle-{cpu_id}"),
            state: ThreadState::Running,
            context: Context::zero(),
            kernel_stack: Vec::new(),
            vruntime: u64::MAX,
            is_user: false,
            first_run: false,
            address_space: None,
        }
    }

    pub fn new_user(name: &str, pid: Pid, entry: u64, user_sp: u64, aspace: AddressSpace) -> Self {
        let mut stack = Vec::with_capacity(KERNEL_STACK_SIZE);
        stack.resize(KERNEL_STACK_SIZE, 0u8);
        let kstack_top = stack.as_ptr() as u64 + KERNEL_STACK_SIZE as u64;

        let mut ctx = Context::zero();
        ctx.x30 = crate::arch::user_trampoline_addr();
        ctx.sp = kstack_top;
        ctx.elr = entry;
        ctx.spsr = 0x0; // SPSR EL0t — interrupts enabled, return to EL0
        ctx.sp_el0 = user_sp;
        ctx.ttbr0 = aspace.root_phys;

        Thread {
            pid,
            name: String::from(name),
            state: ThreadState::Ready,
            context: ctx,
            kernel_stack: stack,
            vruntime: 0,
            is_user: true,
            first_run: true,
            address_space: Some(aspace),
        }
    }
}

pub struct Process {
    pub pid: Pid,
    pub tgid: Pid,
    pub ppid: Pid,
    pub name: String,
    pub threads: Vec<Pid>,
    pub vmm: alloc::sync::Arc<Mutex<Vmm>>,
}

/// Minimal VMM stub for /proc/self/maps.
pub struct Vmm;

impl Vmm {
    pub fn maps_string(&self) -> String {
        String::new()
    }
}

/// Return the PID running on the current CPU.
pub fn current_pid() -> Pid {
    super::cpu::get_current_pid()
}

use alloc::collections::BTreeMap;
use spin::Mutex;
pub static PROCESS_TABLE: Mutex<BTreeMap<Pid, Process>> = Mutex::new(BTreeMap::new());

