// mm/page_table.rs — cross-architecture page table abstraction
//
// Re-exports the architecture-specific AddressSpace as the canonical
// PageTable type.  Code outside the arch/ tree should use these aliases
// so that porting to a new architecture only requires updating this file.

#[cfg(target_arch = "aarch64")]
pub use crate::arch::aarch64::paging::{
    AddressSpace as PageTable,
    PageFlags,
};

fn hhdm_offset() -> u64 {
    crate::arch::hhdm_offset()
}

#[cfg(target_arch = "x86_64")]
pub use crate::arch::x86_64::paging::{
    AddressSpace as PageTable,
    PageFlags,
};

/// Map a single page `phys` → `virt` with the given flags inside `table`.
/// This is a thin wrapper so callers don't need to know the concrete type.
#[allow(dead_code)]
pub fn map_page(table: &PageTable, virt: u64, phys: u64, flags: PageFlags) {
    #[cfg(target_arch = "aarch64")]
    crate::mm::mmap::map_page_in_ttbr0(
        table.root_phys,
        virt,
        phys,
        flags,
        hhdm_offset(),
    );
}
