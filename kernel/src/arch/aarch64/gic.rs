// GICv2 (Generic Interrupt Controller v2) driver for AArch64
// QEMU virt machine default GIC

const GICD_PHYS: usize = 0x0800_0000; // Distributor
const GICC_PHYS: usize = 0x0801_0000; // CPU Interface

fn gicd_base() -> usize {
    GICD_PHYS + super::hhdm_offset() as usize
}

fn gicc_base() -> usize {
    GICC_PHYS + super::hhdm_offset() as usize
}

// Distributor registers
const GICD_CTLR: usize = 0x0000;
const GICD_ISENABLER: usize = 0x0100;
const GICD_ICENABLER: usize = 0x0180;
const GICD_IPRIORITYR: usize = 0x0400;
const GICD_ITARGETSR: usize = 0x0800;
const GICD_ICFGR: usize = 0x0C00;
const GICD_SGIR: usize = 0x0F00;

// CPU Interface registers
const GICC_CTLR: usize = 0x0000;
const GICC_PMR: usize = 0x0004;
const GICC_IAR: usize = 0x000C;
const GICC_EOIR: usize = 0x0010;

// Timer interrupt IDs
pub const TIMER_IRQ: u32 = 30; // Non-secure EL1 physical timer (PPI 14 = INTID 30)
pub const SGI_RESCHEDULE: u32 = 1; // SGI for rescheduling IPI

unsafe fn write_reg(base: usize, offset: usize, val: u32) {
    unsafe { core::ptr::write_volatile((base + offset) as *mut u32, val) };
}

unsafe fn read_reg(base: usize, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base + offset) as *const u32) }
}

pub fn init_distributor() {
    log::debug!("GICv2 GICD at 0x{:x}, GICC at 0x{:x}", gicd_base(), gicc_base());
    unsafe {
        // Disable distributor
        write_reg(gicd_base(), GICD_CTLR, 0);

        // Set all SPIs to target CPU 0, priority 0xA0
        // SPIs start at INTID 32
        for i in 8..32 {
            write_reg(gicd_base(), GICD_IPRIORITYR + i * 4, 0xA0A0A0A0);
        }
        for i in 8..32 {
            write_reg(gicd_base(), GICD_ITARGETSR + i * 4, 0x01010101);
        }

        // Enable distributor (EnableGrp0 + EnableGrp1)
        write_reg(gicd_base(), GICD_CTLR, 3);
    }
    log::debug!("GICv2 distributor initialized");
}

pub fn init_cpu_interface() {
    unsafe {
        // Set priority for SGIs/PPIs (INTID 0-31)
        for i in 0..8 {
            write_reg(gicd_base(), GICD_IPRIORITYR + i * 4, 0xA0A0A0A0);
        }

        // Enable timer PPI (INTID 30) and SGI 1
        write_reg(gicd_base(), GICD_ISENABLER, (1 << TIMER_IRQ) | (1 << SGI_RESCHEDULE));

        // CPU Interface: set priority mask to accept all, enable
        write_reg(gicc_base(), GICC_PMR, 0xFF);
        write_reg(gicc_base(), GICC_CTLR, 1);
    }
    log::debug!("GICv2 CPU interface initialized");
}

/// Acknowledge and return interrupt ID
pub fn ack_interrupt() -> u32 {
    unsafe { read_reg(gicc_base(), GICC_IAR) & 0x3FF }
}

/// Signal end of interrupt
pub fn end_interrupt(intid: u32) {
    unsafe { write_reg(gicc_base(), GICC_EOIR, intid) };
}

/// Send SGI (Software Generated Interrupt) to a specific CPU
pub fn send_sgi(target_cpu: usize, sgi_id: u32) {
    // GICD_SGIR: TargetListFilter=0 (use target list), CPUTargetList, SGIINTID
    let val = ((1u32 << target_cpu as u32) << 16) | (sgi_id & 0xF);
    unsafe {
        write_reg(gicd_base(), GICD_SGIR, val);
    }
}

/// Enable a specific SPI (Shared Peripheral Interrupt)
#[allow(dead_code)]
pub fn enable_irq(intid: u32) {
    let reg = (intid / 32) as usize;
    let bit = intid % 32;
    unsafe {
        write_reg(gicd_base(), GICD_ISENABLER + reg * 4, 1 << bit);
    }
}

/// Disable a specific interrupt
#[allow(dead_code)]
pub fn disable_irq(intid: u32) {
    let reg = (intid / 32) as usize;
    let bit = intid % 32;
    unsafe {
        write_reg(gicd_base(), GICD_ICENABLER + reg * 4, 1 << bit);
    }
}
