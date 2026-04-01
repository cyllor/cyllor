extern crate alloc;

use crate::arch::Arch;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub mod context;
pub mod exceptions;
pub mod gic;
pub mod paging;
pub mod timer;

pub struct Aarch64Arch;

static HHDM_OFFSET: AtomicU64 = AtomicU64::new(0);

#[used]
#[unsafe(link_section = ".requests")]
static HHDM_REQUEST: limine::request::HhdmRequest = limine::request::HhdmRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MEMMAP_REQUEST: limine::request::MemmapRequest = limine::request::MemmapRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MP_REQUEST: limine::request::MpRequest = limine::request::MpRequest::new(0);

#[used]
#[unsafe(link_section = ".requests_start_marker")]
static _START_MARKER: limine::RequestsStartMarker = limine::RequestsStartMarker::new();

#[used]
#[unsafe(link_section = ".requests_end_marker")]
static _END_MARKER: limine::RequestsEndMarker = limine::RequestsEndMarker::new();

pub fn early_init() {
    let hhdm = HHDM_REQUEST
        .response()
        .expect("HHDM response not available");
    HHDM_OFFSET.store(hhdm.offset, Ordering::Relaxed);
    log::debug!("HHDM offset: 0x{:016x}", hhdm.offset);
}

#[allow(dead_code)]
pub fn hhdm_offset() -> u64 {
    HHDM_OFFSET.load(Ordering::Relaxed)
}

pub fn memory_map() -> &'static [&'static limine::memmap::Entry] {
    MEMMAP_REQUEST
        .response()
        .expect("Memory map not available")
        .entries()
}

pub fn cpu_count() -> usize {
    MP_REQUEST
        .response()
        .map(|r| r.cpus().len())
        .unwrap_or(1)
}

/// Return the logical CPU ID from MPIDR_EL1.Aff0.
pub fn current_cpu_id() -> usize {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr) };
    (mpidr & 0xFF) as usize
}

/// Read the AArch64 virtual counter (CNTVCT_EL0).
pub fn read_counter() -> u64 {
    let val: u64;
    unsafe { core::arch::asm!("mrs {}, CNTVCT_EL0", out(reg) val) };
    val
}

/// Read the counter frequency (CNTFRQ_EL0).
pub fn counter_freq() -> u64 {
    let freq: u64;
    unsafe { core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) freq) };
    freq
}

/// Read TTBR0_EL1 and return the physical page-table root (ASID bits masked out).
pub fn read_user_page_table_root() -> u64 {
    let val: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) val) };
    val & 0x0000_FFFF_FFFF_F000
}

/// Switch the user-mode page table (TTBR0_EL1) without a full TLB flush.
pub fn activate_user_page_table(root_phys: u64) {
    unsafe { core::arch::asm!("msr TTBR0_EL1, {0}", "isb", in(reg) root_phys) };
}

/// Send a reschedule SGI to the target CPU.
pub fn send_resched_ipi(target_cpu: usize) {
    gic::send_sgi(target_cpu, gic::SGI_RESCHEDULE);
}

/// Enable an interrupt by INTID.
pub fn enable_irq(intid: u32) {
    gic::enable_irq(intid);
}

/// Enable the UART0 receive interrupt.
pub fn enable_uart_irq() {
    gic::enable_irq(gic::UART0_IRQ);
}

/// Disable IRQs so the caller can enter a critical section uninterrupted.
pub fn mask_irqs() {
    unsafe { core::arch::asm!("msr DAIFSet, #2") };
}

/// Inner-shareable data synchronization barrier.
pub fn data_sync_barrier() {
    unsafe { core::arch::asm!("dsb ish") };
}

impl Arch for Aarch64Arch {
    fn init_interrupts() {
        exceptions::init();
        gic::init_distributor();
        gic::init_redistributor();
        gic::init_cpu_interface();
        timer::init();

        unsafe {
            core::arch::asm!("msr daifclr, #0xf");
        }
    }

    fn enable_interrupts() {
        unsafe {
            core::arch::asm!("msr daifclr, #0xf");
        }
    }

    fn disable_interrupts() {
        unsafe {
            core::arch::asm!("msr daifset, #0xf");
        }
    }

    fn halt() {
        unsafe {
            core::arch::asm!("wfe");
        }
    }
}

// --------------- SMP bring-up ---------------

/// Per-CPU tick counters (for Phase 1 verification)
pub static PER_CPU_TICKS: [AtomicU64; 4] = [
    AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0),
];

static AP_ENTERED: AtomicU32 = AtomicU32::new(0);
static AP_ONLINE: AtomicU32 = AtomicU32::new(0);
static AP_TTBR1_VAL: AtomicU64 = AtomicU64::new(0);
static AP_SCTLR_VAL: AtomicU64 = AtomicU64::new(0);
static AP_TCR_VAL: AtomicU64 = AtomicU64::new(0);
static AP_MAIR_VAL: AtomicU64 = AtomicU64::new(0);
static AP_HHDM_VAL: AtomicU64 = AtomicU64::new(0);
static AP_MMIO_OK: AtomicU32 = AtomicU32::new(0);
/// AP init progress: each AP stores last completed step (1-5)
static AP_STEP: [AtomicU32; 4] = [
    AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0),
];

/// Start secondary CPUs
pub fn start_secondary_cpus() {
    let mp = match MP_REQUEST.response() {
        Some(r) => r,
        None => {
            log::warn!("SMP not available");
            return;
        }
    };

    let cpus = mp.cpus();
    let bsp_mpidr = mp.bsp_mpidr;
    log::info!("SMP: {} CPUs detected, BSP mpidr=0x{:x}", cpus.len(), bsp_mpidr);

    // Allocate kernel stacks in HHDM-mapped heap, pass stack top as extra_argument
    let stacks = alloc_ap_stacks(cpus.len());

    for (i, cpu) in cpus.iter().enumerate() {
        let mpidr = cpu.mpidr;
        if mpidr == bsp_mpidr {
            continue;
        }
        let stack_top = stacks[i.min(3)];
        log::debug!("SMP: bootstrapping CPU[{i}] mpidr=0x{mpidr:x} stack=0x{stack_top:x}");
        cpu.bootstrap(ap_entry, stack_top);
    }

    // Wait for all APs
    let expected = cpus.len() as u32 - 1;
    let mut timeout = 100_000_000u64;
    while AP_ONLINE.load(Ordering::Acquire) < expected && timeout > 0 {
        core::hint::spin_loop();
        timeout -= 1;
    }
    let entered = AP_ENTERED.load(Ordering::Acquire);
    let online = AP_ONLINE.load(Ordering::Acquire);
    let bsp_ttbr1: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR1_EL1", out(reg) bsp_ttbr1) };
    let ap_ttbr1 = AP_TTBR1_VAL.load(Ordering::Acquire);
    let mmio_ok = AP_MMIO_OK.load(Ordering::Acquire);
    log::info!("SMP: {entered}/{expected} entered, {mmio_ok} MMIO ok, {online}/{expected} fully online");

    let bsp_sctlr: u64;
    let bsp_tcr: u64;
    let bsp_mair: u64;
    unsafe {
        core::arch::asm!("mrs {}, SCTLR_EL1", out(reg) bsp_sctlr);
        core::arch::asm!("mrs {}, TCR_EL1", out(reg) bsp_tcr);
        core::arch::asm!("mrs {}, MAIR_EL1", out(reg) bsp_mair);
    }
    let ap_sctlr = AP_SCTLR_VAL.load(Ordering::Acquire);
    let ap_tcr = AP_TCR_VAL.load(Ordering::Acquire);
    let ap_mair = AP_MAIR_VAL.load(Ordering::Acquire);

    log::info!("  TTBR1: BSP=0x{bsp_ttbr1:x} AP=0x{ap_ttbr1:x} {}",
        if bsp_ttbr1 == ap_ttbr1 { "OK" } else { "MISMATCH" });
    log::info!("  SCTLR: BSP=0x{bsp_sctlr:x} AP=0x{ap_sctlr:x} {}",
        if bsp_sctlr == ap_sctlr { "OK" } else { "MISMATCH" });
    log::info!("  TCR:   BSP=0x{bsp_tcr:x} AP=0x{ap_tcr:x} {}",
        if bsp_tcr == ap_tcr { "OK" } else { "MISMATCH" });
    log::info!("  MAIR:  BSP=0x{bsp_mair:x} AP=0x{ap_mair:x} {}",
        if bsp_mair == ap_mair { "OK" } else { "MISMATCH" });
    let ap_hhdm = AP_HHDM_VAL.load(Ordering::Acquire);
    log::info!("  HHDM:  BSP=0x{:x} AP=0x{ap_hhdm:x} {}",
        hhdm_offset(), if hhdm_offset() == ap_hhdm { "OK" } else { "MISMATCH" });

    // Walk TTBR1 page table to see L1[0] (covers phys 0-1GB incl. GIC/UART)
    let hhdm = hhdm_offset();
    let l0_virt = (bsp_ttbr1 & 0x0000_FFFF_FFFF_F000) + hhdm;
    let l0_entry = unsafe { core::ptr::read_volatile(l0_virt as *const u64) }; // L0[0]
    log::info!("  PageTable L0[0] = 0x{l0_entry:016x}");
    if l0_entry & 0x3 == 0x3 {
        // Table descriptor — follow to L1
        let l1_phys = l0_entry & 0x0000_FFFF_FFFF_F000;
        let l1_virt = l1_phys + hhdm;
        let l1_0 = unsafe { core::ptr::read_volatile(l1_virt as *const u64) }; // L1[0]
        let l1_1 = unsafe { core::ptr::read_volatile((l1_virt + 8) as *const u64) }; // L1[1]
        let l1_2 = unsafe { core::ptr::read_volatile((l1_virt + 16) as *const u64) }; // L1[2]
        log::info!("  PageTable L1[0]=0x{l1_0:016x} L1[1]=0x{l1_1:016x} L1[2]=0x{l1_2:016x}");
        log::info!("    L1[0] type={}", if l1_0 & 0x3 == 0x3 { "table" } else if l1_0 & 0x1 == 0x1 { "block" } else { "INVALID" });
    }
    for i in 1..cpus.len().min(4) {
        let s = AP_STEP[i].load(Ordering::Acquire);
        log::info!("  AP{i}: step {s}/5 (1=VBAR 2=GICR 3=ICC 4=timer 5=IRQ)");
    }
}

/// Print per-CPU tick snapshot (call from BSP after some time)
pub fn dump_per_cpu_ticks() {
    for i in 0..cpu_count().min(4) {
        let t = PER_CPU_TICKS[i].load(Ordering::Relaxed);
        log::info!("  CPU{i}: {t} ticks");
    }
}

/// Allocate kernel stacks for all CPUs. Returns array of stack tops.
fn alloc_ap_stacks(num_cpus: usize) -> [u64; 4] {
    let mut tops = [0u64; 4];
    for i in 0..num_cpus.min(4) {
        let stack = alloc::vec![0u8; 64 * 1024];
        tops[i] = stack.as_ptr() as u64 + stack.len() as u64;
        core::mem::forget(stack); // leak — AP stacks live forever
    }
    tops
}

unsafe extern "C" fn ap_entry(info: &limine::mp::MpInfo) -> ! {
    // extra_argument = stack top in HHDM heap. Switch SP FIRST, before
    // any Rust code that touches the stack (Limine's stack is unmapped).
    let stack_top = info.extra_argument();
    unsafe {
        core::arch::asm!("mov sp, {}", in(reg) stack_top);
        // Enable FP/SIMD — Limine only enables it on BSP
        core::arch::asm!("msr CPACR_EL1, {}", in(reg) 3u64 << 20);
        core::arch::asm!("isb");
    }

    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr) };
    let idx = (mpidr & 0xFF) as usize;
    let idx = idx.min(3);

    AP_ENTERED.fetch_add(1, Ordering::Release);

    exceptions::init();
    AP_STEP[idx].store(1, Ordering::Release); // VBAR set

    let ttbr1: u64;
    let sctlr: u64;
    let tcr: u64;
    let mair: u64;
    unsafe {
        core::arch::asm!("mrs {}, TTBR1_EL1", out(reg) ttbr1);
        core::arch::asm!("mrs {}, SCTLR_EL1", out(reg) sctlr);
        core::arch::asm!("mrs {}, TCR_EL1", out(reg) tcr);
        core::arch::asm!("mrs {}, MAIR_EL1", out(reg) mair);
    }
    AP_TTBR1_VAL.store(ttbr1, Ordering::SeqCst);
    AP_SCTLR_VAL.store(sctlr, Ordering::SeqCst);
    AP_TCR_VAL.store(tcr, Ordering::SeqCst);
    AP_MAIR_VAL.store(mair, Ordering::SeqCst);
    AP_HHDM_VAL.store(hhdm_offset(), Ordering::SeqCst);

    // Store TTBR0 too
    static AP_TTBR0_VAL: AtomicU64 = AtomicU64::new(0xDEAD);
    let ttbr0: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr0) };
    AP_TTBR0_VAL.store(ttbr0, Ordering::SeqCst);

    // Test: can AP write to UART via PHYSICAL address (TTBR0 identity map)?
    static AP_PHYS_MMIO_OK: AtomicU32 = AtomicU32::new(0);
    unsafe {
        core::ptr::write_volatile(0x0900_0000u64 as *mut u8, b'@');
    }
    AP_PHYS_MMIO_OK.fetch_add(1, Ordering::SeqCst);

    // Test: can AP write to UART MMIO via HHDM?
    unsafe {
        let uart = (0x0900_0000u64 + hhdm_offset()) as *mut u8;
        core::ptr::write_volatile(uart, b'!');
    }
    AP_MMIO_OK.fetch_add(1, Ordering::SeqCst);

    gic::init_redistributor();
    AP_STEP[idx].store(2, Ordering::Release); // GICR done

    gic::init_cpu_interface();
    AP_STEP[idx].store(3, Ordering::Release); // ICC done

    timer::init();
    AP_STEP[idx].store(4, Ordering::Release); // timer armed

    unsafe { core::arch::asm!("msr daifclr, #0xf") };
    AP_STEP[idx].store(5, Ordering::Release); // IRQ enabled

    AP_ONLINE.fetch_add(1, Ordering::Release);

    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}
