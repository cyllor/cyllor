// arch/aarch64/context.rs — AArch64 context switch (callee-saved register save/restore)
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
