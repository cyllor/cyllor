// mm/page_table.rs — cross-architecture page table abstraction
//
// Re-exports the architecture-specific AddressSpace as the canonical PageTable
// type via crate::arch.  Porting to a new arch only requires updating crate::arch.

pub use crate::arch::{AddressSpace as PageTable, PageFlags};

/// Map a single page `phys` → `virt` with the given flags inside `table`.
#[allow(dead_code)]
pub fn map_page(table: &PageTable, virt: u64, phys: u64, flags: PageFlags) {
    crate::arch::map_user_page(table.root_phys, virt, phys, flags);
}
