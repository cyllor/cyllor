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
