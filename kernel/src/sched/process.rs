use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::arch::aarch64::paging::AddressSpace;

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
    pub ttbr0: u64,  // user page table
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
            address_space: None,
        }
    }

    pub fn new_user(name: &str, pid: Pid, entry: u64, user_sp: u64, aspace: AddressSpace) -> Self {
        let mut stack = Vec::with_capacity(KERNEL_STACK_SIZE);
        stack.resize(KERNEL_STACK_SIZE, 0u8);
        let kstack_top = stack.as_ptr() as u64 + KERNEL_STACK_SIZE as u64;

        let mut ctx = Context::zero();
        // x30 = return_to_user trampoline
        ctx.x30 = return_to_user_trampoline as u64;
        ctx.sp = kstack_top;
        ctx.elr = entry;
        // SPSR: EL0t (return to EL0 with SP_EL0), all interrupts enabled
        ctx.spsr = 0x0; // EL0t
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
            address_space: Some(aspace),
        }
    }
}

pub struct Process {
    pub pid: Pid,
    pub name: String,
    pub threads: Vec<Pid>,
}

/// Trampoline function: sets up EL0 return and does ERET
/// Called as a "kernel thread" that immediately drops to user mode
#[unsafe(naked)]
unsafe extern "C" fn return_to_user_trampoline() -> ! {
    // At this point:
    // - Thread's context has elr, spsr, sp_el0, ttbr0 set
    // - We need to load these and ERET to userspace
    // The scheduler will have stored context pointer somewhere accessible
    // For now, we use a simple approach: the context is on the kernel stack

    core::arch::naked_asm!(
        // The thread was "context switched" to, so x19-x30 are restored.
        // x19 was set to point to the context by the scheduler setup.
        // But for the trampoline, we stored entry/sp/spsr in the Context struct.
        // We need a different approach: store user entry info in known registers.
        //
        // After context_switch restores x19-x28, x30=this function, sp=kstack.
        // We stored: x19=elr, x20=spsr, x21=sp_el0, x22=ttbr0
        // (Set up by the scheduler before first context switch)

        // Set user page table
        "msr TTBR0_EL1, x22",
        "isb",
        "tlbi vmalle1is",
        "dsb ish",
        "isb",

        // Set up EL0 return
        "msr ELR_EL1, x19",
        "msr SPSR_EL1, x20",
        "msr SP_EL0, x21",

        // Clear all general purpose registers for clean userspace entry
        "mov x0, #0",
        "mov x1, #0",
        "mov x2, #0",
        "mov x3, #0",
        "mov x4, #0",
        "mov x5, #0",
        "mov x6, #0",
        "mov x7, #0",
        "mov x8, #0",
        "mov x9, #0",
        "mov x10, #0",
        "mov x11, #0",
        "mov x12, #0",
        "mov x13, #0",
        "mov x14, #0",
        "mov x15, #0",
        "mov x16, #0",
        "mov x17, #0",
        "mov x18, #0",
        // x19-x22 are used above, clear them too
        "mov x19, #0",
        "mov x20, #0",
        "mov x21, #0",
        "mov x22, #0",
        "mov x23, #0",
        "mov x24, #0",
        "mov x25, #0",
        "mov x26, #0",
        "mov x27, #0",
        "mov x28, #0",
        "mov x29, #0",
        "mov x30, #0",

        "eret",
    );
}
