// GICv3 (Generic Interrupt Controller v3) driver for AArch64
// QEMU virt machine with -M virt,gic-version=3

use core::arch::asm;

// QEMU virt GICv3 memory map
const GICD_PHYS: usize = 0x0800_0000;
const GICR_PHYS: usize = 0x080A_0000;
const GICR_STRIDE: usize = 0x2_0000; // 128KB per redistributor (RD_base + SGI_base)

fn gicd_base() -> usize {
    GICD_PHYS + super::hhdm_offset() as usize
}

fn gicr_rd_base(cpu_id: usize) -> usize {
    GICR_PHYS + super::hhdm_offset() as usize + cpu_id * GICR_STRIDE
}

fn gicr_sgi_base(cpu_id: usize) -> usize {
    gicr_rd_base(cpu_id) + 0x1_0000 // SGI_base = RD_base + 64KB
}

// ---------- Distributor registers ----------
const GICD_CTLR: usize = 0x0000;
const GICD_ISENABLER: usize = 0x0100;
const GICD_ICENABLER: usize = 0x0180;
const GICD_IPRIORITYR: usize = 0x0400;
const GICD_IROUTER: usize = 0x6100; // 64-bit per SPI, starts at INTID 32

// ---------- Redistributor RD_base registers ----------
const GICR_WAKER: usize = 0x0014;

// ---------- Redistributor SGI_base registers (offset +0x10000) ----------
const GICR_ISENABLER0: usize = 0x0100;
const GICR_ICENABLER0: usize = 0x0180;
const GICR_IPRIORITYR0: usize = 0x0400;

// ---------- Well-known interrupt IDs ----------
pub const TIMER_IRQ: u32 = 30; // Non-secure EL1 physical timer (PPI 14 = INTID 30)
pub const UART0_IRQ: u32 = 33; // PL011 UART0 on QEMU virt (SPI 1 = INTID 33)
pub const SGI_RESCHEDULE: u32 = 1;

// ---------- Low-level MMIO helpers ----------
unsafe fn write32(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) };
}

unsafe fn read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

unsafe fn write64(addr: usize, val: u64) {
    unsafe { core::ptr::write_volatile(addr as *mut u64, val) };
}

// ---------- Distributor (one-time, BSP only) ----------

pub fn init_distributor() {
    let base = gicd_base();
    log::debug!("GICv3 GICD at 0x{base:x}");

    unsafe {
        // Disable distributor while configuring
        write32(base + GICD_CTLR, 0);

        // Set default priority 0xA0 for all SPIs (INTID 32..1020)
        for i in 8..32 {
            write32(base + GICD_IPRIORITYR + i * 4, 0xA0A0_A0A0);
        }

        // Route all SPIs to affinity 0.0.0.0 (CPU 0)
        for spi in 0..64u64 {
            write64(base + GICD_IROUTER + spi as usize * 8, 0);
        }

        // Enable distributor: ARE_NS(4) | EnableGrp1NS(1) | EnableGrp0(0)
        write32(base + GICD_CTLR, (1 << 4) | (1 << 1) | (1 << 0));
    }
    log::debug!("GICv3 distributor initialized (affinity routing enabled)");
}

// ---------- Redistributor (per-CPU) ----------

pub fn init_redistributor() {
    let cpu_id = cpu_id();
    let rd = gicr_rd_base(cpu_id);
    let sgi = gicr_sgi_base(cpu_id);

    unsafe {
        // Wake the redistributor: clear ProcessorSleep (bit 1)
        // On QEMU, redistributors may already be awake — use timeout
        let waker = read32(rd + GICR_WAKER);
        if waker & (1 << 1) != 0 {
            write32(rd + GICR_WAKER, waker & !(1 << 1));
            let mut timeout = 1_000_000u32;
            while read32(rd + GICR_WAKER) & (1 << 2) != 0 && timeout > 0 {
                core::hint::spin_loop();
                timeout -= 1;
            }
        }

        // Set priority 0xA0 for SGIs/PPIs (INTID 0..31)
        for i in 0..8 {
            write32(sgi + GICR_IPRIORITYR0 + i * 4, 0xA0A0_A0A0);
        }

        // Enable timer PPI (30) and reschedule SGI (1)
        write32(sgi + GICR_ISENABLER0, (1 << TIMER_IRQ) | (1 << SGI_RESCHEDULE));
    }
}

/// Simplified redistributor init that skips GICR_WAKER entirely.
/// Use this for AP cores where the redistributor may already be awake.
pub fn init_redistributor_ap() {
    let cpu_id = cpu_id();
    let sgi = gicr_sgi_base(cpu_id);

    unsafe {
        for i in 0..8 {
            write32(sgi + GICR_IPRIORITYR0 + i * 4, 0xA0A0_A0A0);
        }
        write32(sgi + GICR_ISENABLER0, (1 << TIMER_IRQ) | (1 << SGI_RESCHEDULE));
    }
}

// ---------- CPU Interface (per-CPU, via ICC system registers) ----------

pub fn init_cpu_interface() {
    unsafe {
        // Enable system register access
        let sre: u64;
        asm!("mrs {}, ICC_SRE_EL1", out(reg) sre);
        asm!("msr ICC_SRE_EL1, {}", in(reg) sre | 0x7); // SRE | DFB | DIB
        asm!("isb");

        // Priority mask: accept all priorities
        asm!("msr ICC_PMR_EL1, {}", in(reg) 0xFFu64);

        // No sub-priority grouping
        asm!("msr ICC_BPR1_EL1, {}", in(reg) 0u64);

        // Enable Group 1 interrupts
        asm!("msr ICC_IGRPEN1_EL1, {}", in(reg) 1u64);

        asm!("isb");
    }
    // log omitted — called from AP context
}

// ---------- Runtime interrupt operations ----------

/// Acknowledge and return interrupt ID (Group 1)
pub fn ack_interrupt() -> u32 {
    let iar: u64;
    unsafe { asm!("mrs {}, ICC_IAR1_EL1", out(reg) iar) };
    (iar & 0xFF_FFFF) as u32
}

/// Signal end of interrupt
pub fn end_interrupt(intid: u32) {
    unsafe { asm!("msr ICC_EOIR1_EL1, {}", in(reg) intid as u64) };
}

/// Send reschedule SGI to a specific CPU.
///
/// ICC_SGI1R_EL1 format (QEMU virt: flat affinity 0.0.0.N):
///   [3:0]   TargetList — bitmask of target PEs within Aff0
///   [27:24] INTID
///   [39:32] Aff2  (0)
///   [47:40] Aff3  (0)
///   [23:16] Aff1  (0)
pub fn send_sgi(target_cpu: usize, sgi_id: u32) {
    let val: u64 = ((sgi_id as u64 & 0xF) << 24) | (1u64 << (target_cpu & 0xF));
    unsafe { asm!("msr ICC_SGI1R_EL1, {}", in(reg) val) };
}

/// Enable a specific interrupt
pub fn enable_irq(intid: u32) {
    if intid < 32 {
        // SGI / PPI — per-CPU, via redistributor
        let sgi = gicr_sgi_base(cpu_id());
        unsafe { write32(sgi + GICR_ISENABLER0, 1 << intid) };
    } else {
        // SPI — shared, via distributor
        let reg = (intid / 32) as usize;
        let bit = intid % 32;
        unsafe { write32(gicd_base() + GICD_ISENABLER + reg * 4, 1 << bit) };
    }
}

/// Disable a specific interrupt
#[allow(dead_code)]
pub fn disable_irq(intid: u32) {
    if intid < 32 {
        let sgi = gicr_sgi_base(cpu_id());
        unsafe { write32(sgi + GICR_ICENABLER0, 1 << intid) };
    } else {
        let reg = (intid / 32) as usize;
        let bit = intid % 32;
        unsafe { write32(gicd_base() + GICD_ICENABLER + reg * 4, 1 << bit) };
    }
}

// ---------- Helpers ----------

fn cpu_id() -> usize {
    let mpidr: u64;
    unsafe { asm!("mrs {}, MPIDR_EL1", out(reg) mpidr) };
    (mpidr & 0xFF) as usize
}
