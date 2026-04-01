use limine::memmap::MEMMAP_USABLE;
use spin::Mutex;

const PAGE_SIZE: usize = 4096;
const MAX_PAGES: usize = 1024 * 1024; // Support up to 4 GiB

static PMM: Mutex<BitmapAllocator> = Mutex::new(BitmapAllocator::new());

struct BitmapAllocator {
    bitmap: [u64; MAX_PAGES / 64],
    total_pages: usize,
    free_pages: usize,
}

impl BitmapAllocator {
    const fn new() -> Self {
        Self {
            bitmap: [0xFFFF_FFFF_FFFF_FFFF; MAX_PAGES / 64],
            total_pages: 0,
            free_pages: 0,
        }
    }

    fn mark_free(&mut self, page: usize) {
        let idx = page / 64;
        let bit = page % 64;
        self.bitmap[idx] &= !(1u64 << bit);
        self.free_pages += 1;
    }

    fn alloc_page(&mut self) -> Option<usize> {
        for (i, entry) in self.bitmap.iter_mut().enumerate() {
            if *entry != 0xFFFF_FFFF_FFFF_FFFF {
                let bit = (!*entry).trailing_zeros() as usize;
                *entry |= 1u64 << bit;
                self.free_pages -= 1;
                return Some((i * 64 + bit) * PAGE_SIZE);
            }
        }
        None
    }

    #[allow(dead_code)]
    fn free_page(&mut self, phys_addr: usize) {
        let page = phys_addr / PAGE_SIZE;
        let idx = page / 64;
        let bit = page % 64;
        self.bitmap[idx] &= !(1u64 << bit);
        self.free_pages += 1;
    }
}

pub fn init() {
    let entries = crate::arch::memory_map();
    let mut pmm = PMM.lock();
    let mut total = 0usize;

    // Only use pages that are covered by Limine's HHDM mapping.
    // QEMU virt with -m 1G may have usable memory above 1 GB (UEFI layout),
    // but Limine's TTBR1 HHDM typically only maps contiguous physical RAM.
    // We limit to 2 GB to be safe — adjust if needed for larger RAM configs.
    const MAX_PHYS: usize = 2 * 1024 * 1024 * 1024; // 2 GB

    for entry in entries {
        if entry.type_ == MEMMAP_USABLE {
            let base = entry.base as usize;
            let len = entry.length as usize;
            let end = base + len;
            if end > MAX_PHYS {
                log::warn!("PMM: skipping region 0x{:x}-0x{:x} (above HHDM limit)", base, end);
                if base >= MAX_PHYS { continue; }
            }
            let usable_end = end.min(MAX_PHYS);
            let start_page = base / PAGE_SIZE;
            let page_count = (usable_end - base) / PAGE_SIZE;

            for p in start_page..start_page + page_count {
                if p < MAX_PAGES {
                    pmm.mark_free(p);
                    total += 1;
                }
            }
        }
    }

    pmm.total_pages = total;
    log::info!(
        "PMM: {} MiB usable ({} pages)",
        total * PAGE_SIZE / (1024 * 1024),
        total
    );
}

#[allow(dead_code)]
pub fn mark_used(phys_addr: usize) {
    let mut pmm = PMM.lock();
    let page = phys_addr / PAGE_SIZE;
    if page < MAX_PAGES {
        let idx = page / 64;
        let bit = page % 64;
        if pmm.bitmap[idx] & (1u64 << bit) == 0 {
            // Was free, mark as used
            pmm.bitmap[idx] |= 1u64 << bit;
            if pmm.free_pages > 0 { pmm.free_pages -= 1; }
        }
    }
}

pub fn alloc_page() -> Option<usize> {
    PMM.lock().alloc_page()
}

#[allow(dead_code)]
pub fn free_page(phys_addr: usize) {
    PMM.lock().free_page(phys_addr);
}
