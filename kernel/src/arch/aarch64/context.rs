// arch/aarch64/context.rs — AArch64 context switch and user-mode entry
use core::arch::global_asm;
use crate::sched::process::Context;

global_asm!(
    ".global context_switch_asm",
    "context_switch_asm:",
    // Save callee-saved registers of old thread into *old (x0)
    "stp x19, x20, [x0, #0]",
    "stp x21, x22, [x0, #16]",
    "stp x23, x24, [x0, #32]",
    "stp x25, x26, [x0, #48]",
    "stp x27, x28, [x0, #64]",
    "stp x29, x30, [x0, #80]",
    "mov x2, sp",
    "str x2, [x0, #96]",
    // Restore callee-saved registers of new thread from *new (x1)
    "ldp x19, x20, [x1, #0]",
    "ldp x21, x22, [x1, #16]",
    "ldp x23, x24, [x1, #32]",
    "ldp x25, x26, [x1, #48]",
    "ldp x27, x28, [x1, #64]",
    "ldp x29, x30, [x1, #80]",
    "ldr x2, [x1, #96]",
    "mov sp, x2",
    "ret",

    // switch_to_new_asm: restore new context and jump (no old context to save)
    ".global switch_to_new_asm",
    "switch_to_new_asm:",
    "ldp x19, x20, [x0, #0]",
    "ldp x21, x22, [x0, #16]",
    "ldp x23, x24, [x0, #32]",
    "ldp x25, x26, [x0, #48]",
    "ldp x27, x28, [x0, #64]",
    "ldp x29, x30, [x0, #80]",
    "ldr x1, [x0, #96]",
    "mov sp, x1",
    "ret",
);

unsafe extern "C" {
    pub fn context_switch_asm(old: *mut Context, new: *const Context);
    pub fn switch_to_new_asm(new: *const Context);
}

/// Save `old` callee-saved context and resume `new`.
pub unsafe fn context_switch(old: &mut Context, new: &Context) {
    unsafe { context_switch_asm(old, new) }
}

/// Restore `new` context and jump into it (first run / idle→thread switch).
pub unsafe fn switch_to_new(new: &Context) {
    unsafe { switch_to_new_asm(new) }
}

/// Trampoline: called on a user thread's very first context switch.
/// x19=entry, x20=SPSR, x21=SP_EL0, x22=TTBR0 (set by scheduler before switch).
#[unsafe(naked)]
pub unsafe extern "C" fn return_to_user_trampoline() -> ! {
    core::arch::naked_asm!(
        "msr TTBR0_EL1, x22",
        "isb",
        "msr ELR_EL1, x19",
        "msr SPSR_EL1, x20",
        "msr SPSel, #0",
        "mov sp, x21",
        "msr SPSel, #1",
        "mov x0,  #0", "mov x1,  #0", "mov x2,  #0", "mov x3,  #0",
        "mov x4,  #0", "mov x5,  #0", "mov x6,  #0", "mov x7,  #0",
        "mov x8,  #0", "mov x9,  #0", "mov x10, #0", "mov x11, #0",
        "mov x12, #0", "mov x13, #0", "mov x14, #0", "mov x15, #0",
        "mov x16, #0", "mov x17, #0", "mov x18, #0", "mov x19, #0",
        "mov x20, #0", "mov x21, #0", "mov x22, #0", "mov x23, #0",
        "mov x24, #0", "mov x25, #0", "mov x26, #0", "mov x27, #0",
        "mov x28, #0", "mov x29, #0", "mov x30, #0",
        "eret",
    );
}

/// Return the address of the user-mode entry trampoline.
pub fn user_trampoline_addr() -> u64 {
    return_to_user_trampoline as *const () as u64
}
