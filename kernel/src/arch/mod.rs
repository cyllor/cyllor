#[allow(dead_code)]
pub trait Arch {
    fn init_interrupts();
    fn enable_interrupts();
    fn disable_interrupts();
    fn halt();
}

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use self::x86_64::X86Arch as PlatformArch;
#[cfg(target_arch = "x86_64")]
pub use self::x86_64::{early_init, memory_map};
#[cfg(target_arch = "x86_64")]
pub use self::x86_64::{TrapFrame, AddressSpace, PageFlags};
#[cfg(target_arch = "x86_64")]
pub const ARCH_NAME: &str = "x86_64";
#[cfg(target_arch = "x86_64")]
pub fn cpu_count() -> usize { 1 } // TODO
/// ELF e_machine value for the current architecture.
#[cfg(target_arch = "x86_64")]
pub const ELF_MACHINE: u16 = 62;  // EM_X86_64
/// glibc dynamic linker filename for this architecture.
#[cfg(target_arch = "x86_64")]
pub const INTERP_NAME: &str = "ld-linux-x86-64.so.2";
/// GNU multiarch library directory name for this architecture.
#[cfg(target_arch = "x86_64")]
pub const GNU_LIB_DIR: &str = "x86_64-linux-gnu";
/// PCIe ECAM physical base address (0 = must discover via ACPI MCFG).
#[cfg(target_arch = "x86_64")]
pub const PCI_ECAM_BASE: u64 = 0;

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use self::aarch64::Aarch64Arch as PlatformArch;
#[cfg(target_arch = "aarch64")]
pub use self::aarch64::{early_init, memory_map};
#[cfg(target_arch = "aarch64")]
pub const ARCH_NAME: &str = "aarch64";
#[cfg(target_arch = "aarch64")]
pub use self::aarch64::cpu_count;
#[cfg(target_arch = "aarch64")]
pub fn ticks() -> u64 { aarch64::exceptions::ticks() }
#[cfg(target_arch = "aarch64")]
pub fn hhdm_offset() -> u64 { aarch64::hhdm_offset() }
#[cfg(target_arch = "aarch64")]
pub use self::aarch64::paging::{AddressSpace, PageFlags};
/// ELF e_machine value for the current architecture.
#[cfg(target_arch = "aarch64")]
pub const ELF_MACHINE: u16 = 183; // EM_AARCH64
/// glibc dynamic linker filename for this architecture.
#[cfg(target_arch = "aarch64")]
pub const INTERP_NAME: &str = "ld-linux-aarch64.so.1";
/// GNU multiarch library directory name for this architecture.
#[cfg(target_arch = "aarch64")]
pub const GNU_LIB_DIR: &str = "aarch64-linux-gnu";
/// PCIe ECAM physical base address on QEMU virt AArch64.
#[cfg(target_arch = "aarch64")]
pub const PCI_ECAM_BASE: u64 = 0x4010_0000_0000;

/// VirtIO MMIO device region base address (platform-specific).
#[cfg(target_arch = "aarch64")]
pub const VIRTIO_MMIO_BASE: usize = 0x0a00_0000; // QEMU virt AArch64
/// VirtIO MMIO device stride (bytes between consecutive devices).
pub const VIRTIO_MMIO_STRIDE: usize = 0x200;
#[cfg(target_arch = "x86_64")]
pub const VIRTIO_MMIO_BASE: usize = 0; // VirtIO on x86_64 typically uses PCI, not MMIO

/// User virtual address where the brk (heap) region starts.
pub const USER_BRK_BASE: usize = 0x0000_0060_0000_0000;    // 384 GiB
/// User virtual address where anonymous mmap allocations start.
pub const USER_MMAP_BASE: usize = 0x0000_0070_0000_0000;   // 448 GiB
/// User stack top address.
pub const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;

#[cfg(target_arch = "aarch64")]
pub fn current_cpu_id() -> usize { aarch64::current_cpu_id() }
#[cfg(not(target_arch = "aarch64"))]
pub fn current_cpu_id() -> usize { 0 }
#[cfg(target_arch = "aarch64")]
pub fn read_counter() -> u64 { aarch64::read_counter() }
#[cfg(not(target_arch = "aarch64"))]
pub fn read_counter() -> u64 { 0 }
#[cfg(target_arch = "aarch64")]
pub fn activate_user_page_table(root_phys: u64) { aarch64::activate_user_page_table(root_phys) }
#[cfg(not(target_arch = "aarch64"))]
pub fn activate_user_page_table(_root_phys: u64) {}

#[cfg(target_arch = "aarch64")]
pub fn counter_freq() -> u64 { aarch64::counter_freq() }
#[cfg(not(target_arch = "aarch64"))]
pub fn counter_freq() -> u64 { 1 }

#[cfg(target_arch = "aarch64")]
pub fn read_user_page_table_root() -> u64 { aarch64::read_user_page_table_root() }
#[cfg(not(target_arch = "aarch64"))]
pub fn read_user_page_table_root() -> u64 { 0 }

#[cfg(target_arch = "aarch64")]
pub fn send_resched_ipi(target_cpu: usize) { aarch64::send_resched_ipi(target_cpu) }
#[cfg(not(target_arch = "aarch64"))]
pub fn send_resched_ipi(_target_cpu: usize) {}

#[cfg(target_arch = "aarch64")]
pub fn enable_irq(intid: u32) { aarch64::enable_irq(intid) }
#[cfg(not(target_arch = "aarch64"))]
pub fn enable_irq(_intid: u32) {}

#[cfg(target_arch = "aarch64")]
pub fn enable_uart_irq() { aarch64::enable_uart_irq() }
#[cfg(not(target_arch = "aarch64"))]
pub fn enable_uart_irq() {}

#[cfg(target_arch = "aarch64")]
pub fn mask_irqs() { aarch64::mask_irqs() }
#[cfg(not(target_arch = "aarch64"))]
pub fn mask_irqs() {}

#[cfg(target_arch = "aarch64")]
pub fn data_sync_barrier() { aarch64::data_sync_barrier() }
#[cfg(not(target_arch = "aarch64"))]
pub fn data_sync_barrier() {}

#[cfg(target_arch = "aarch64")]
pub fn init_mair() { aarch64::paging::init_mair() }
#[cfg(not(target_arch = "aarch64"))]
pub fn init_mair() {}

#[cfg(target_arch = "aarch64")]
pub fn start_secondary_cpus() { aarch64::start_secondary_cpus() }
#[cfg(not(target_arch = "aarch64"))]
pub fn start_secondary_cpus() {}

#[cfg(target_arch = "aarch64")]
pub fn dump_per_cpu_ticks() { aarch64::dump_per_cpu_ticks() }
#[cfg(not(target_arch = "aarch64"))]
pub fn dump_per_cpu_ticks() {}

#[cfg(target_arch = "aarch64")]
pub fn user_trampoline_addr() -> u64 { aarch64::context::user_trampoline_addr() }
#[cfg(not(target_arch = "aarch64"))]
pub fn user_trampoline_addr() -> u64 { 0 }

#[cfg(target_arch = "aarch64")]
pub use aarch64::exceptions::TrapFrame;

#[cfg(target_arch = "aarch64")]
pub unsafe fn context_switch_raw(
    old: *mut crate::sched::process::Context,
    new: *const crate::sched::process::Context,
) {
    unsafe { aarch64::context::context_switch_asm(old, new) }
}
#[cfg(not(target_arch = "aarch64"))]
pub unsafe fn context_switch_raw(
    _old: *mut crate::sched::process::Context,
    _new: *const crate::sched::process::Context,
) {}

#[cfg(target_arch = "aarch64")]
pub unsafe fn switch_to_new_raw(new: *const crate::sched::process::Context) {
    unsafe { aarch64::context::switch_to_new_asm(new) }
}
#[cfg(not(target_arch = "aarch64"))]
pub unsafe fn switch_to_new_raw(_new: *const crate::sched::process::Context) {}

/// Translate a user virtual address (given page table root PA) to physical address.
#[cfg(target_arch = "aarch64")]
pub fn translate_user_va(root_phys: u64, va: u64) -> Option<u64> {
    aarch64::paging::translate_user_va(root_phys, va)
}
#[cfg(not(target_arch = "aarch64"))]
pub fn translate_user_va(_root_phys: u64, _va: u64) -> Option<u64> { None }

/// Write a single byte to the platform UART.
#[cfg(target_arch = "aarch64")]
pub fn uart_write_byte(byte: u8) { aarch64::uart_write_byte(byte) }
#[cfg(not(target_arch = "aarch64"))]
pub fn uart_write_byte(byte: u8) { x86_64::uart_write_byte(byte) }

/// Initialize the platform UART.
#[cfg(target_arch = "aarch64")]
pub fn uart_init() { aarch64::uart_init() }
#[cfg(not(target_arch = "aarch64"))]
pub fn uart_init() { x86_64::uart_init() }

/// Enable UART receive interrupt.
#[cfg(target_arch = "aarch64")]
pub fn uart_enable_rx_interrupt() { aarch64::uart_enable_rx_interrupt() }
#[cfg(not(target_arch = "aarch64"))]
pub fn uart_enable_rx_interrupt() {} // TODO: COM1 RX interrupt on x86_64

/// Handle UART receive interrupt — drain FIFO, calling push_byte for each byte.
#[cfg(target_arch = "aarch64")]
pub fn uart_handle_rx_interrupt(push_byte: fn(u8)) { aarch64::uart_handle_rx_interrupt(push_byte) }
#[cfg(not(target_arch = "aarch64"))]
pub fn uart_handle_rx_interrupt(_push_byte: fn(u8)) {}

/// Map a single user page (phys → virt) in the page table rooted at root_phys.
#[cfg(target_arch = "aarch64")]
pub fn map_user_page(root_phys: u64, virt: u64, phys: u64, flags: PageFlags) {
    aarch64::paging::map_user_page(root_phys, virt, phys, flags)
}
#[cfg(not(target_arch = "aarch64"))]
pub fn map_user_page(_root_phys: u64, _virt: u64, _phys: u64, _flags: PageFlags) {}
