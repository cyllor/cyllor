// AArch64 4-level page table management (4KB granule, 48-bit VA)
//
// Virtual address layout:
//   [63:48] = 0xFFFF -> kernel (TTBR1_EL1)
//   [63:48] = 0x0000 -> user   (TTBR0_EL1)
//
// Page table levels (4KB granule):
//   L0: bits [47:39] -> 512 entries, each covers 512GB
//   L1: bits [38:30] -> 512 entries, each covers 1GB
//   L2: bits [29:21] -> 512 entries, each covers 2MB
//   L3: bits [20:12] -> 512 entries, each covers 4KB (page)

use crate::mm::pmm;
use crate::arch::{PageAttr, PageTable as ArchPageTable};
use core::ptr;

const PAGE_SIZE: usize = 4096;
const ENTRIES_PER_TABLE: usize = 512;

// Page table entry flags
const PTE_VALID: u64 = 1 << 0;
const PTE_TABLE: u64 = 1 << 1; // For L0-L2: points to next table
const PTE_PAGE: u64 = 1 << 1;  // For L3: this is a page
const PTE_AF: u64 = 1 << 10;   // Access flag
const PTE_SH_INNER: u64 = 3 << 8; // Inner shareable
const PTE_AP_RW_EL1: u64 = 0 << 6; // R/W at EL1 only
const PTE_AP_RW_ALL: u64 = 1 << 6; // R/W at EL0+EL1
const PTE_AP_RO_EL1: u64 = 2 << 6; // RO at EL1 only
const PTE_AP_RO_ALL: u64 = 3 << 6; // RO at EL0+EL1
const PTE_UXN: u64 = 1 << 54; // Unprivileged execute-never
const PTE_PXN: u64 = 1 << 53; // Privileged execute-never
const PTE_ATTR_NORMAL: u64 = 0 << 2; // MAIR index 0: Normal memory
const PTE_ATTR_DEVICE: u64 = 1 << 2; // MAIR index 1: Device-nGnRnE
const PTE_NG: u64 = 1 << 11;  // Non-global (for user pages, use ASID)

fn hhdm_offset() -> u64 {
    super::hhdm_offset()
}

/// Convert physical to virtual (via HHDM)
fn phys_to_virt(phys: u64) -> u64 {
    phys + hhdm_offset()
}

/// Convert HHDM virtual to physical
fn virt_to_phys(virt: u64) -> u64 {
    virt - hhdm_offset()
}

/// Allocate a zeroed page for a page table, return physical address
fn alloc_table_page() -> Option<u64> {
    let phys = pmm::alloc_page()? as u64;
    let virt = phys_to_virt(phys);
    unsafe { ptr::write_bytes(virt as *mut u8, 0, PAGE_SIZE); }
    Some(phys)
}

/// A page table (L0/L1/L2/L3) — 512 entries of 8 bytes
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [u64; ENTRIES_PER_TABLE],
}

/// User address space — owns the root L0 page table
pub struct AddressSpace {
    pub root_phys: u64,  // Physical address of L0 table
}

impl AddressSpace {
    /// Create a new empty user address space
    pub fn new() -> Option<Self> {
        let root = alloc_table_page()?;
        Some(Self { root_phys: root })
    }

    /// Map a virtual page to a physical page with given flags
    pub fn map_page(&self, virt: u64, phys: u64, flags: PageAttr) -> Result<(), ()> {
        let pte_flags = PageFlags::from(flags).to_pte();
        let indices = [
            ((virt >> 39) & 0x1FF) as usize, // L0
            ((virt >> 30) & 0x1FF) as usize, // L1
            ((virt >> 21) & 0x1FF) as usize, // L2
            ((virt >> 12) & 0x1FF) as usize, // L3
        ];

        let mut table_phys = self.root_phys;

        // Walk L0 -> L1 -> L2, creating intermediate tables as needed
        for level in 0..3 {
            let table_virt = phys_to_virt(table_phys) as *mut u64;
            let entry = unsafe { ptr::read_volatile(table_virt.add(indices[level])) };

            if entry & PTE_VALID == 0 {
                let new_table = alloc_table_page().ok_or(())?;
                if new_table == self.root_phys {
                    // PMM returned the root table page — BUG!
                    return Err(());
                }
                let new_entry = new_table | PTE_VALID | PTE_TABLE;
                unsafe { ptr::write_volatile(table_virt.add(indices[level]), new_entry); }
                // Verify write
                let verify = unsafe { ptr::read_volatile(table_virt.add(indices[level])) };
                if verify != new_entry {
                    crate::drivers::uart::early_print("PAGE TABLE WRITE FAILED!\n");
                    return Err(());
                }
                table_phys = new_table;
            } else {
                table_phys = entry & 0x0000_FFFF_FFFF_F000;
            }
        }

        // Write L3 entry (page mapping)
        let l3_virt = phys_to_virt(table_phys) as *mut u64;
        let l3_entry = (phys & 0x0000_FFFF_FFFF_F000) | pte_flags | PTE_VALID | PTE_PAGE | PTE_AF;
        unsafe { ptr::write_volatile(l3_virt.add(indices[3]), l3_entry); }

        Ok(())
    }

    /// Map a contiguous range of virtual pages to physical pages
    pub fn map_range(&self, virt_start: u64, phys_start: u64, size: usize, flags: PageAttr) -> Result<(), ()> {
        let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        for i in 0..pages {
            let offset = (i * PAGE_SIZE) as u64;
            self.map_page(virt_start + offset, phys_start + offset, flags)?;
        }
        Ok(())
    }

    /// Map anonymous pages (allocate physical memory)
    pub fn map_anon(&self, virt_start: u64, size: usize, flags: PageAttr) -> Result<(), ()> {
        let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        log::debug!("map_anon: virt=0x{:x} pages={} root=0x{:x}", virt_start, pages, self.root_phys);
        for i in 0..pages {
            if pages > 100 && i % 512 == 0 {
                crate::drivers::uart::write_byte(b'.');
            }
            let phys = pmm::alloc_page().ok_or(())? as u64;
            // Zero the page
            let virt_phys = phys_to_virt(phys);
            unsafe { ptr::write_bytes(virt_phys as *mut u8, 0, PAGE_SIZE); }
            self.map_page(virt_start + (i * PAGE_SIZE) as u64, phys, flags)?;
        }
        Ok(())
    }

    /// Unmap a page, returning the physical address
    pub fn unmap_page(&self, virt: u64) -> Option<u64> {
        let indices = [
            ((virt >> 39) & 0x1FF) as usize,
            ((virt >> 30) & 0x1FF) as usize,
            ((virt >> 21) & 0x1FF) as usize,
            ((virt >> 12) & 0x1FF) as usize,
        ];

        let mut table_phys = self.root_phys;

        for level in 0..3 {
            let table_virt = phys_to_virt(table_phys) as *const u64;
            let entry = unsafe { ptr::read_volatile(table_virt.add(indices[level])) };
            if entry & PTE_VALID == 0 {
                return None;
            }
            table_phys = entry & 0x0000_FFFF_FFFF_F000;
        }

        let l3_virt = phys_to_virt(table_phys) as *mut u64;
        let entry = unsafe { ptr::read_volatile(l3_virt.add(indices[3])) };
        if entry & PTE_VALID == 0 {
            return None;
        }

        // Clear the entry
        unsafe { ptr::write_volatile(l3_virt.add(indices[3]), 0); }
        // Invalidate TLB for this address
        unsafe { core::arch::asm!("tlbi vale1is, {}", in(reg) virt >> 12); }

        Some(entry & 0x0000_FFFF_FFFF_F000)
    }

    /// Switch to this address space (set TTBR0_EL1)
    pub fn activate(&self) {
        unsafe {
            core::arch::asm!(
                "msr TTBR0_EL1, {}",
                "isb",
                "tlbi vmalle1is",
                "dsb ish",
                "isb",
                in(reg) self.root_phys,
            );
        }
    }

    /// Translate a user virtual address to physical
    pub fn translate(&self, virt: u64) -> Option<u64> {
        let indices = [
            ((virt >> 39) & 0x1FF) as usize,
            ((virt >> 30) & 0x1FF) as usize,
            ((virt >> 21) & 0x1FF) as usize,
            ((virt >> 12) & 0x1FF) as usize,
        ];

        let mut table_phys = self.root_phys;

        for level in 0..3 {
            let table_virt = phys_to_virt(table_phys) as *const u64;
            let entry = unsafe { ptr::read_volatile(table_virt.add(indices[level])) };
            if entry & PTE_VALID == 0 {
                return None;
            }
            table_phys = entry & 0x0000_FFFF_FFFF_F000;
        }

        let l3_virt = phys_to_virt(table_phys) as *const u64;
        let entry = unsafe { ptr::read_volatile(l3_virt.add(indices[3])) };
        if entry & PTE_VALID == 0 {
            return None;
        }

        let page_phys = entry & 0x0000_FFFF_FFFF_F000;
        Some(page_phys | (virt & 0xFFF))
    }

    /// Copy data from kernel buffer to user virtual address
    pub fn copy_to_user(&self, user_virt: u64, data: &[u8]) -> Result<(), ()> {
        let mut offset = 0usize;
        while offset < data.len() {
            let page_offset = ((user_virt + offset as u64) & 0xFFF) as usize;
            let chunk = (PAGE_SIZE - page_offset).min(data.len() - offset);
            let phys = self.translate(user_virt + offset as u64).ok_or(())?;
            let kern_virt = phys_to_virt(phys);
            unsafe {
                ptr::copy_nonoverlapping(
                    data[offset..].as_ptr(),
                    kern_virt as *mut u8,
                    chunk,
                );
            }
            offset += chunk;
        }
        Ok(())
    }

    /// Copy data from user virtual address to kernel buffer
    pub fn copy_from_user(&self, user_virt: u64, buf: &mut [u8]) -> Result<(), ()> {
        let mut offset = 0usize;
        while offset < buf.len() {
            let page_offset = ((user_virt + offset as u64) & 0xFFF) as usize;
            let chunk = (PAGE_SIZE - page_offset).min(buf.len() - offset);
            let phys = self.translate(user_virt + offset as u64).ok_or(())?;
            let kern_virt = phys_to_virt(phys);
            unsafe {
                ptr::copy_nonoverlapping(
                    kern_virt as *const u8,
                    buf[offset..].as_mut_ptr(),
                    chunk,
                );
            }
            offset += chunk;
        }
        Ok(())
    }
}

impl ArchPageTable for AddressSpace {
    fn root_phys(&self) -> u64 { self.root_phys }

    fn map_anon(&self, virt_start: u64, size: usize, flags: PageAttr) -> Result<(), ()> {
        AddressSpace::map_anon(self, virt_start, size, flags)
    }

    fn copy_to_user(&self, virt: u64, data: &[u8]) -> Result<(), ()> {
        AddressSpace::copy_to_user(self, virt, data)
    }

    fn copy_from_user(&self, virt: u64, buf: &mut [u8]) -> Result<(), ()> {
        AddressSpace::copy_from_user(self, virt, buf)
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        // TODO: walk and free all page table pages and mapped pages
        // For now, leak them
    }
}

/// Page mapping flags (architecture-independent)
#[derive(Debug, Clone, Copy)]
pub struct PageFlags {
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
    pub user: bool,
    pub device: bool,
}

impl From<PageAttr> for PageFlags {
    fn from(attr: PageAttr) -> Self {
        Self {
            readable: attr.readable,
            writable: attr.writable,
            executable: attr.executable,
            user: attr.user,
            device: attr.device,
        }
    }
}

impl PageFlags {
    pub const USER_RWX: Self = Self { readable: true, writable: true, executable: true, user: true, device: false };
    pub const USER_RW: Self = Self { readable: true, writable: true, executable: false, user: true, device: false };
    pub const USER_RX: Self = Self { readable: true, writable: false, executable: true, user: true, device: false };
    pub const USER_RO: Self = Self { readable: true, writable: false, executable: false, user: true, device: false };
    pub const KERNEL_RW: Self = Self { readable: true, writable: true, executable: false, user: false, device: false };
    pub const KERNEL_RWX: Self = Self { readable: true, writable: true, executable: true, user: false, device: false };
    pub const DEVICE: Self = Self { readable: true, writable: true, executable: false, user: false, device: true };

    pub fn to_pte(self) -> u64 {
        let mut flags: u64 = PTE_SH_INNER;

        if self.device {
            flags |= PTE_ATTR_DEVICE;
        } else {
            flags |= PTE_ATTR_NORMAL;
        }

        if self.user {
            flags |= PTE_NG; // Non-global for user pages
            if self.writable {
                flags |= PTE_AP_RW_ALL;
            } else {
                flags |= PTE_AP_RO_ALL;
            }
            if !self.executable {
                flags |= PTE_UXN;
            }
            flags |= PTE_PXN; // Never execute user pages in kernel
        } else {
            if self.writable {
                flags |= PTE_AP_RW_EL1;
            } else {
                flags |= PTE_AP_RO_EL1;
            }
            if !self.executable {
                flags |= PTE_PXN;
            }
            flags |= PTE_UXN;
        }

        flags
    }
}

/// Fix MAIR_EL1 so AttrIndx=1 → Device-nGnRnE (0x00) instead of Normal.
/// Limine sets MAIR=0xFFFF (both index 0 and 1 are Normal=0xFF).
/// We need: index 0 = 0xFF (Normal WB), index 1 = 0x00 (Device-nGnRnE).
/// Must be called on EVERY CPU (BSP + all APs) before device MMIO access.
pub fn fix_mair() {
    let old_mair: u64;
    unsafe { core::arch::asm!("mrs {}, MAIR_EL1", out(reg) old_mair) };

    // Keep index 0 (Normal=0xFF), set index 1 to Device-nGnRnE (0x00), rest unchanged
    let new_mair = (old_mair & !0xFF00) | (0x00 << 8); // index 1 = 0x00 = Device

    unsafe {
        core::arch::asm!("msr MAIR_EL1, {}", in(reg) new_mair);
        core::arch::asm!("isb");
        // Must invalidate TLB since MAIR interpretation changed
        core::arch::asm!(
            "dsb ish",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
        );
    }
}

pub fn init_mair() {
    let tcr: u64;
    let mair: u64;
    unsafe {
        core::arch::asm!("mrs {}, TCR_EL1", out(reg) tcr);
        core::arch::asm!("mrs {}, MAIR_EL1", out(reg) mair);
    }
    log::debug!("Limine TCR_EL1=0x{tcr:016x} MAIR_EL1=0x{mair:016x}");
}

/// Fix Limine's HHDM mapping for device MMIO regions.
/// Limine maps everything as Normal memory, but device MMIO needs Device-nGnRnE attributes.
/// For QEMU virt, phys 0-1GB has NO RAM (only devices), so we can just rewrite
/// the L1 block entry with Device attributes.
pub fn fix_device_mmio_attrs() {
    let hhdm = super::hhdm_offset();
    let ttbr1: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR1_EL1", out(reg) ttbr1) };
    let l0_phys = ttbr1 & 0x0000_FFFF_FFFF_F000;

    // Walk L0[0] → L1
    let l0_virt = (l0_phys + hhdm) as *const u64;
    let l0_entry = unsafe { ptr::read_volatile(l0_virt) };
    if l0_entry & 0x3 != 0x3 {
        return; // L0[0] not a table descriptor, can't fix
    }
    let l1_phys = l0_entry & 0x0000_FFFF_FFFF_F000;
    let l1_virt = (l1_phys + hhdm) as *mut u64;

    // L1[0] covers phys 0x0000_0000 - 0x3FFF_FFFF (1GB)
    // This range on QEMU virt contains GIC (0x0800_0000), UART (0x0900_0000), VirtIO (0x0A00_0000)
    // No RAM here, so we can safely mark it all as Device memory.
    let l1_0 = unsafe { ptr::read_volatile(l1_virt) };
    if l1_0 & 0x1 == 0x1 && l1_0 & 0x2 == 0 {
        // It's a 1GB block entry — rewrite with Device-nGnRnE attributes
        // Keep: Valid(0)=1, Block(1)=0, AF(10)=1, phys addr, SH=Inner Shareable
        // Change: AttrIndx from 0 (Normal) to 1 (Device-nGnRnE)
        let phys_addr = l1_0 & 0x0000_FFFF_C000_0000; // L1 block alignment: 1GB
        let new_entry = phys_addr
            | (1 << 0)     // Valid
            // bit 1 = 0: block descriptor
            | PTE_ATTR_DEVICE  // AttrIndx = 1
            | PTE_SH_INNER    // Inner shareable
            | (1 << 10)       // AF
            | PTE_PXN         // No kernel exec from device
            | PTE_UXN;        // No user exec from device
        unsafe {
            ptr::write_volatile(l1_virt, new_entry);
            core::arch::asm!(
                "dsb ish",
                "tlbi vmalle1is",
                "dsb ish",
                "isb",
            );
        }
        log::info!("Fixed L1[0]: 0x{l1_0:016x} → 0x{new_entry:016x} (Normal → Device)");
    }
}

/// Translate a user virtual address using an arbitrary page table root (physical address).
pub fn translate_user_va(root_phys: u64, va: u64) -> Option<u64> {
    let aspace = core::mem::ManuallyDrop::new(AddressSpace { root_phys });
    aspace.translate(va)
}

/// Map a single user page (phys → virt) in the page table rooted at root_phys.
pub fn map_user_page(root_phys: u64, virt: u64, phys: u64, flags: PageAttr) {
    let aspace = core::mem::ManuallyDrop::new(AddressSpace { root_phys });
    let _ = aspace.map_page(virt, phys, flags.into());
    super::data_sync_barrier();
}

/// Unmap a single user page in the page table rooted at root_phys.
/// Returns the unmapped physical page base when a mapping existed.
pub fn unmap_user_page(root_phys: u64, virt: u64) -> Option<u64> {
    let aspace = core::mem::ManuallyDrop::new(AddressSpace { root_phys });
    let phys = aspace.unmap_page(virt);
    super::data_sync_barrier();
    phys
}

/// Invalidate TLB entry for one user virtual page.
pub fn flush_user_tlb_va(virt: u64) {
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi vale1is, {}",
            "dsb ish",
            "isb",
            in(reg) virt >> 12,
        );
    }
}

/// Map a physical device MMIO range into TTBR1 (kernel HHDM space).
/// Limine's HHDM may not include device MMIO regions — this ensures
/// GIC, UART etc. are accessible from all CPUs via TTBR1.
pub fn map_device_mmio(phys_start: u64, size: usize) {
    let hhdm = super::hhdm_offset();
    let ttbr1: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR1_EL1", out(reg) ttbr1) };
    let l0_phys = ttbr1 & 0x0000_FFFF_FFFF_F000;

    let flags = PageFlags::DEVICE;
    let pte_flags = flags.to_pte();
    let mut addr = phys_start & !0xFFF;
    let end = (phys_start + size as u64 + 0xFFF) & !0xFFF;

    while addr < end {
        let virt = addr + hhdm;
        // Walk TTBR1 page tables (kernel uses upper VA range, bit 63:48 = 0xFFFF)
        // For TTBR1, we strip the top bits to get indices
        let indices = [
            ((virt >> 39) & 0x1FF) as usize,
            ((virt >> 30) & 0x1FF) as usize,
            ((virt >> 21) & 0x1FF) as usize,
            ((virt >> 12) & 0x1FF) as usize,
        ];

        let mut table_phys = l0_phys;

        // Walk/create L0 → L1 → L2
        for level in 0..3 {
            let table_virt = (table_phys + hhdm) as *mut u64;
            let entry = unsafe { ptr::read_volatile(table_virt.add(indices[level])) };

            if entry & 0x3 == 0x3 {
                // Valid table entry — follow it
                table_phys = entry & 0x0000_FFFF_FFFF_F000;
            } else if entry & 0x1 == 0x1 && entry & 0x2 == 0 && level > 0 {
                // Block entry (1GB or 2MB) — page already mapped via block
                // The address is covered by a large mapping, no need to add L3
                break;
            } else {
                // Not present — allocate a new page table
                let new_table = pmm::alloc_page().unwrap() as u64;
                unsafe { ptr::write_bytes((new_table + hhdm) as *mut u8, 0, PAGE_SIZE); }
                let new_entry = new_table | 0x3; // Valid + Table
                unsafe { ptr::write_volatile(table_virt.add(indices[level]), new_entry); }
                table_phys = new_table;
            }
        }

        // Check if already mapped (by a block entry above)
        let l3_virt = (table_phys + hhdm) as *mut u64;
        let existing = unsafe { ptr::read_volatile(l3_virt.add(indices[3])) };
        if existing & 1 == 0 {
            // Not mapped — add L3 page entry
            let entry = (addr & 0x0000_FFFF_FFFF_F000) | pte_flags | 0x3 | (1 << 10);
            unsafe { ptr::write_volatile(l3_virt.add(indices[3]), entry); }
        }

        addr += PAGE_SIZE as u64;
    }

    // TLB + barrier
    unsafe {
        core::arch::asm!(
            "dsb ish",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
        );
    }
}
