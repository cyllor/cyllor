// mm/page_table.rs – cross-architecture page table abstraction
//
// Keep helper API architecture-neutral through crate::arch traits.

pub use crate::arch::{PageAttr, PageTable};

/// Map a single page `phys` → `virt` with the given flags inside `table`.
#[allow(dead_code)]
pub fn map_page(table: &dyn PageTable, virt: u64, phys: u64, flags: PageAttr) {
    crate::arch::map_user_page(table.root_phys(), virt, phys, flags);
}
