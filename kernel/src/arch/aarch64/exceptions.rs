use core::arch::global_asm;
use super::gic;
use super::timer;

/// Trap frame saved on exception entry
#[repr(C)]
pub struct TrapFrame {
    pub regs: [u64; 31], // x0-x30
    pub sp: u64,
    pub elr: u64,        // Exception Link Register
    pub spsr: u64,       // Saved Program Status Register
}

// Exception vector table - must be aligned to 2048 bytes
global_asm!(
    ".align 11",
    "exception_vector_table:",

    // Current EL with SP0
    ".align 7", "b sync_handler",      // Synchronous
    ".align 7", "b irq_handler",       // IRQ
    ".align 7", "b unhandled_exception", // FIQ
    ".align 7", "b unhandled_exception", // SError

    // Current EL with SPx
    ".align 7", "b sync_handler",
    ".align 7", "b irq_handler",
    ".align 7", "b unhandled_exception",
    ".align 7", "b unhandled_exception",

    // Lower EL using AArch64
    ".align 7", "b lower_sync_handler",
    ".align 7", "b lower_irq_handler",
    ".align 7", "b unhandled_exception",
    ".align 7", "b unhandled_exception",

    // Lower EL using AArch32
    ".align 7", "b unhandled_exception",
    ".align 7", "b unhandled_exception",
    ".align 7", "b unhandled_exception",
    ".align 7", "b unhandled_exception",

    // Save registers macro
    ".macro SAVE_REGS",
    "sub sp, sp, #272",       // 31 regs + sp + elr + spsr = 34 * 8 = 272
    "stp x0, x1, [sp, #0]",
    "stp x2, x3, [sp, #16]",
    "stp x4, x5, [sp, #32]",
    "stp x6, x7, [sp, #48]",
    "stp x8, x9, [sp, #64]",
    "stp x10, x11, [sp, #80]",
    "stp x12, x13, [sp, #96]",
    "stp x14, x15, [sp, #112]",
    "stp x16, x17, [sp, #128]",
    "stp x18, x19, [sp, #144]",
    "stp x20, x21, [sp, #160]",
    "stp x22, x23, [sp, #176]",
    "stp x24, x25, [sp, #192]",
    "stp x26, x27, [sp, #208]",
    "stp x28, x29, [sp, #224]",
    "str x30, [sp, #240]",
    "mrs x0, elr_el1",
    "mrs x1, spsr_el1",
    "stp x0, x1, [sp, #256]",  // elr, spsr
    ".endm",

    // Restore registers macro
    ".macro RESTORE_REGS",
    "ldp x0, x1, [sp, #256]",
    "msr elr_el1, x0",
    "msr spsr_el1, x1",
    "ldp x0, x1, [sp, #0]",
    "ldp x2, x3, [sp, #16]",
    "ldp x4, x5, [sp, #32]",
    "ldp x6, x7, [sp, #48]",
    "ldp x8, x9, [sp, #64]",
    "ldp x10, x11, [sp, #80]",
    "ldp x12, x13, [sp, #96]",
    "ldp x14, x15, [sp, #112]",
    "ldp x16, x17, [sp, #128]",
    "ldp x18, x19, [sp, #144]",
    "ldp x20, x21, [sp, #160]",
    "ldp x22, x23, [sp, #176]",
    "ldp x24, x25, [sp, #192]",
    "ldp x26, x27, [sp, #208]",
    "ldp x28, x29, [sp, #224]",
    "ldr x30, [sp, #240]",
    "add sp, sp, #272",
    ".endm",

    // IRQ handler (kernel mode)
    "irq_handler:",
    "SAVE_REGS",
    "mov x0, sp",
    "bl irq_handler_rust",
    "RESTORE_REGS",
    "eret",

    // Sync handler (kernel mode)
    "sync_handler:",
    "SAVE_REGS",
    "mov x0, sp",
    "bl sync_handler_rust",
    "RESTORE_REGS",
    "eret",

    // Lower EL IRQ handler (from userspace)
    "lower_irq_handler:",
    "SAVE_REGS",
    "mov x0, sp",
    "bl irq_handler_rust",
    "RESTORE_REGS",
    "eret",

    // Lower EL Sync handler (syscalls + faults from userspace)
    "lower_sync_handler:",
    "SAVE_REGS",
    "mov x0, sp",
    "bl lower_sync_handler_rust",
    "RESTORE_REGS",
    "eret",

    "unhandled_exception:",
    "SAVE_REGS",
    "mov x0, sp",
    "bl unhandled_exception_rust",
    "b .",
);

pub fn init() {
    unsafe {
        core::arch::asm!(
            "adr x0, exception_vector_table",
            "msr VBAR_EL1, x0",
            "isb",
            out("x0") _,
        );
    }
    log::debug!("Exception vector table installed");
}

static TICK_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn ticks() -> u64 {
    TICK_COUNT.load(core::sync::atomic::Ordering::Relaxed)
}

#[unsafe(no_mangle)]
extern "C" fn irq_handler_rust(_frame: *mut TrapFrame) {
    let intid = gic::ack_interrupt();

    match intid {
        gic::TIMER_IRQ => {
            timer::reset();
            let tick = TICK_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
            if tick % 10 == 0 {
                log::trace!("Timer tick {tick}");
            }
            // Phase 3: trigger scheduler here
            crate::sched::timer_tick();
        }
        gic::SGI_RESCHEDULE => {
            // IPI for rescheduling
            crate::sched::timer_tick();
        }
        1020..=1023 => {
            // Spurious interrupt, ignore
            return;
        }
        _ => {
            log::warn!("Unhandled IRQ: {intid}");
        }
    }

    gic::end_interrupt(intid);
}

#[unsafe(no_mangle)]
extern "C" fn sync_handler_rust(frame: *mut TrapFrame) {
    let esr: u64;
    unsafe { core::arch::asm!("mrs {}, ESR_EL1", out(reg) esr) };
    let ec = (esr >> 26) & 0x3F;
    let elr = unsafe { (*frame).elr };

    match ec {
        0x15 => {
            // SVC (syscall from EL0)
            crate::syscall::handle(unsafe { &mut *frame });
        }
        _ => {
            let far: u64;
            unsafe { core::arch::asm!("mrs {}, FAR_EL1", out(reg) far) };
            log::error!("Sync exception: EC=0x{ec:02x} ESR=0x{esr:016x} ELR=0x{elr:016x} FAR=0x{far:016x}");
            loop { core::hint::spin_loop(); }
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn lower_sync_handler_rust(frame: *mut TrapFrame) {
    sync_handler_rust(frame);
}

#[unsafe(no_mangle)]
extern "C" fn unhandled_exception_rust(_frame: *mut TrapFrame) {
    let esr: u64;
    let elr: u64;
    unsafe {
        core::arch::asm!("mrs {}, ESR_EL1", out(reg) esr);
        core::arch::asm!("mrs {}, ELR_EL1", out(reg) elr);
    }
    log::error!("Unhandled exception: ESR=0x{esr:016x} ELR=0x{elr:016x}");
    loop { core::hint::spin_loop(); }
}
