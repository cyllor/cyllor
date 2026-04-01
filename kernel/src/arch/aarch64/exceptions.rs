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

// Exception vector table
global_asm!(
    ".align 11",
    "exception_vector_table:",
    // Current EL with SP0
    ".align 7", "b sync_handler",
    ".align 7", "b irq_handler",
    ".align 7", "b unhandled_exception",
    ".align 7", "b unhandled_exception",
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

    ".macro SAVE_REGS",
    "sub sp, sp, #272",
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
    "stp x0, x1, [sp, #256]",
    ".endm",

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

    "irq_handler:",
    "SAVE_REGS",
    "mov x0, sp",
    "bl irq_handler_rust",
    "RESTORE_REGS",
    "eret",

    "sync_handler:",
    "SAVE_REGS",
    "mov x0, sp",
    "bl sync_handler_rust",
    "RESTORE_REGS",
    "eret",

    "lower_irq_handler:",
    "SAVE_REGS",
    "mov x0, sp",
    "bl lower_irq_handler_rust",
    "RESTORE_REGS",
    "eret",

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

unsafe extern "C" {
    fn exception_vector_table();
}

pub fn init() {
    unsafe {
        let tbl = exception_vector_table as usize as u64;
        core::arch::asm!(
            "msr VBAR_EL1, {0}",
            "isb",
            in(reg) tbl,
        );
    }
    // Only log from BSP
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr) };
    if mpidr & 0xFF == 0 {
        log::debug!("Exception vector table installed");
    }
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
            TICK_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // Per-CPU tick counter
            let cpu = cpu_id();
            if cpu < 4 {
                super::PER_CPU_TICKS[cpu].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
            // Only BSP runs the scheduler (Phase 1)
            if cpu == 0 {
                crate::sched::timer_tick();
            }
        }
        gic::SGI_RESCHEDULE => {
            // Phase 1: ignore SGI on APs
            if cpu_id() == 0 {
                crate::sched::timer_tick();
            }
        }
        gic::UART0_IRQ => {
            crate::drivers::uart::handle_rx_interrupt();
        }
        1020..=1023 => return,
        _ => {}
    }
    gic::end_interrupt(intid);
}

fn cpu_id() -> usize {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr) };
    (mpidr & 0xFF) as usize
}

#[unsafe(no_mangle)]
extern "C" fn lower_irq_handler_rust(_frame: *mut TrapFrame) {
    let intid = gic::ack_interrupt();
    match intid {
        gic::TIMER_IRQ => {
            timer::reset();
            TICK_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            let cpu = cpu_id();
            if cpu < 4 {
                super::PER_CPU_TICKS[cpu].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
            if cpu == 0 {
                crate::sched::timer_tick();
            }
        }
        gic::SGI_RESCHEDULE => {
            if cpu_id() == 0 {
                crate::sched::timer_tick();
            }
        }
        gic::UART0_IRQ => {
            crate::drivers::uart::handle_rx_interrupt();
        }
        1020..=1023 => return,
        _ => {}
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
            crate::drivers::uart::early_print("!KS ec=");
            print_dec(ec as u64);
            crate::drivers::uart::early_print(" far=");
            print_dec(far);
            crate::drivers::uart::early_print(" elr=");
            print_dec(elr);
            crate::drivers::uart::early_print("\n");
            loop { core::hint::spin_loop(); }
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn lower_sync_handler_rust(frame: *mut TrapFrame) {
    static LSC: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let _n = LSC.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let esr: u64;
    unsafe { core::arch::asm!("mrs {}, ESR_EL1", out(reg) esr) };
    let ec = (esr >> 26) & 0x3F;

    if ec != 0x15 {
        let far: u64;
        unsafe { core::arch::asm!("mrs {}, FAR_EL1", out(reg) far) };
        let elr = unsafe { (*frame).elr };
        crate::drivers::uart::early_print("LE:");
        print_dec(ec);
        crate::drivers::uart::early_print(" elr=");
        print_dec(elr);
        crate::drivers::uart::early_print(" far=");
        print_dec(far);
        crate::drivers::uart::early_print("\n");
    }

    match ec {
        0x15 => {
            // SVC (syscall)
            crate::syscall::handle(unsafe { &mut *frame });
        }
        0x20 | 0x21 => {
            // Instruction Abort from lower EL
            let far: u64;
            unsafe { core::arch::asm!("mrs {}, FAR_EL1", out(reg) far) };
            if !handle_page_fault(far, false) {
                let elr = unsafe { (*frame).elr };
                log::error!("FATAL: User iabort: ELR=0x{elr:016x} FAR=0x{far:016x}");
                loop { core::hint::spin_loop(); }
            }
        }
        0x24 | 0x25 => {
            // Data Abort from lower EL
            let far: u64;
            unsafe { core::arch::asm!("mrs {}, FAR_EL1", out(reg) far) };
            let is_write = (esr >> 6) & 1 != 0;
            if !handle_page_fault(far, is_write) {
                let elr = unsafe { (*frame).elr };
                log::error!("FATAL: User dabort: ELR=0x{elr:016x} FAR=0x{far:016x} w={is_write}");
                loop { core::hint::spin_loop(); }
            }
        }
        _ => {
            let far: u64;
            unsafe { core::arch::asm!("mrs {}, FAR_EL1", out(reg) far) };
            let elr = unsafe { (*frame).elr };
            log::error!("FATAL: User exc: EC=0x{ec:02x} ELR=0x{elr:016x} FAR=0x{far:016x}");
            loop { core::hint::spin_loop(); }
        }
    }
}

fn print_dec(val: u64) {
    if val > 0x7FFF_FFFF_FFFF_0000 {
        // Negative (error code)
        crate::drivers::uart::early_print("-");
        let neg = (-(val as i64)) as u64;
        print_dec(neg);
        return;
    }
    if val >= 0x1000 {
        // Print as hex for large values
        crate::drivers::uart::early_print("0x");
        for i in (0..16).rev() {
            let nibble = ((val >> (i * 4)) & 0xF) as u8;
            if nibble != 0 || i < 8 {
                let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
                crate::drivers::uart::write_byte(c);
            }
        }
        return;
    }
    let mut buf = [0u8; 20];
    let mut n = val;
    let mut i = 19;
    loop {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 { break; }
        if i == 0 { break; }
        i -= 1;
    }
    for &b in &buf[i..] {
        crate::drivers::uart::write_byte(b);
    }
}

/// Handle a page fault by demand-paging: allocate a zero page and map it
fn handle_page_fault(far: u64, _is_write: bool) -> bool {
    let page_addr = far & !0xFFF;

    // Reject NULL pointer dereferences and low addresses
    if far < 0x1000 {
        return false; // Real NULL deref, crash the process
    }
    // Only allow demand paging in valid user regions
    // Code: 0x400000-0x500000, Stack: 0x7FFF..., mmap: 0x7000..., brk: 0x6000..., interp: 0x5000...
    if far < 0x400000 && far >= 0x1000 {
        return false; // Not a valid mapped region
    }

    // Get current TTBR0
    let ttbr0: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr0) };
    let l0_phys = ttbr0 & 0x0000_FFFF_FFFF_F000;
    let hhdm = crate::arch::aarch64::hhdm_offset();

    // Check if the page is already mapped (stale TLB)
    // Walk the page table
    let indices = [
        ((page_addr >> 39) & 0x1FF) as usize,
        ((page_addr >> 30) & 0x1FF) as usize,
        ((page_addr >> 21) & 0x1FF) as usize,
        ((page_addr >> 12) & 0x1FF) as usize,
    ];

    let mut table_phys = l0_phys;
    for level in 0..3 {
        let table_virt = (table_phys + hhdm) as *const u64;
        let entry = unsafe { core::ptr::read_volatile(table_virt.add(indices[level])) };
        if entry & 1 == 0 {
            // Need to allocate — do demand paging
            break;
        }
        table_phys = entry & 0x0000_FFFF_FFFF_F000;
    }

    static PF_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let count = PF_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if count < 20 {
        crate::drivers::uart::early_print("pf:");
        print_dec(page_addr);
        crate::drivers::uart::early_print("\n");
    }
    if count > 10000 {
        crate::drivers::uart::early_print("Too many page faults!\n");
        return false;
    }

    // Allocate and map a zero page
    let phys = match crate::mm::pmm::alloc_page() {
        Some(p) => p as u64,
        None => return false,
    };
    unsafe { core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, 4096); }

    let flags = crate::arch::aarch64::paging::PageFlags::USER_RW;
    crate::mm::mmap::map_page_in_ttbr0(l0_phys, page_addr, phys, flags, hhdm);

    // Flush TLB for this address
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi vale1is, {}",
            "dsb ish",
            "isb",
            in(reg) page_addr >> 12,
        );
    }

    true
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
