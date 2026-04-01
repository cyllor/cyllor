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
pub const ARCH_NAME: &str = "x86_64";
#[cfg(target_arch = "x86_64")]
pub fn cpu_count() -> usize { 1 } // TODO

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
