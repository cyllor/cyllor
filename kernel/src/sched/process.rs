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
    // Field names match the AArch64 ABI (x19-x30, sp). On x86_64 these
    // would correspond to rbx, rbp, r12-r15 etc. — redefine per arch.
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
    pub user_pc: u64,           // user entry point (program counter)
    pub user_flags: u64,        // user processor status flags
    pub user_sp: u64,           // user stack pointer
    pub page_table_root: u64,   // user page-table root (physical address)
}

impl Context {
    pub const fn zero() -> Self {
        Self {
            x19: 0, x20: 0, x21: 0, x22: 0, x23: 0, x24: 0,
            x25: 0, x26: 0, x27: 0, x28: 0, x29: 0, x30: 0,
            sp: 0, user_pc: 0, user_flags: 0, user_sp: 0, page_table_root: 0,
        }
    }

    /// Set the return address for context_switch (link register on AArch64).
    pub fn set_return_addr(&mut self, addr: u64) {
        self.x30 = addr;
    }

    /// Prepare a user-mode thread for its first context switch.
    /// Loads user-mode fields into callee-saved registers so the arch
    /// trampoline can perform the privilege transition.
    #[cfg(target_arch = "aarch64")]
    pub fn prepare_first_run(&mut self) {
        self.x19 = self.user_pc;
        self.x20 = self.user_flags;
        self.x21 = self.user_sp;
        self.x22 = self.page_table_root;
    }

    #[cfg(not(target_arch = "aarch64"))]
    pub fn prepare_first_run(&mut self) {
        // TODO: x86_64 first-run setup
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
    /// The scheduler calls context.prepare_first_run() before clearing this flag.
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
        ctx.set_return_addr(entry as u64);
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
        ctx.set_return_addr(crate::arch::user_trampoline_addr());
        ctx.sp = kstack_top;
        ctx.user_pc = entry;
        ctx.user_flags = 0x0; // Interrupts enabled, return to user mode
        ctx.user_sp = user_sp;
        ctx.page_table_root = aspace.root_phys;

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

