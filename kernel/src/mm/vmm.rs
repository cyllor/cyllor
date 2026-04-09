// Virtual Memory Manager (VMM)
// Tracks per-process Virtual Memory Areas (VMAs) for mmap/munmap/mprotect
// and /proc/self/maps generation.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

// mmap prot flags
pub const PROT_NONE:  u32 = 0;
pub const PROT_READ:  u32 = 1;
pub const PROT_WRITE: u32 = 2;
pub const PROT_EXEC:  u32 = 4;

// mmap flags
pub const MAP_SHARED:    u32 = 0x01;
pub const MAP_PRIVATE:   u32 = 0x02;
pub const MAP_FIXED:     u32 = 0x10;
pub const MAP_ANONYMOUS: u32 = 0x20;

/// A Virtual Memory Area — one contiguous region in the process address space.
#[derive(Debug, Clone)]
pub struct Vma {
    /// Start virtual address (page-aligned, inclusive).
    pub start: u64,
    /// End virtual address (page-aligned, exclusive).
    pub end: u64,
    /// Protection flags (PROT_READ | PROT_WRITE | PROT_EXEC).
    pub prot: u32,
    /// Mapping flags (MAP_PRIVATE | MAP_SHARED | MAP_ANONYMOUS …).
    pub flags: u32,
    /// File offset for file-backed mappings; 0 for anonymous.
    pub offset: u64,
    /// Backing name — file path or special region name (e.g. "[stack]", "[heap]").
    pub name: Option<String>,
}

impl Vma {
    pub fn new_anon(start: u64, end: u64, prot: u32, name: Option<&str>) -> Self {
        Self {
            start,
            end,
            prot,
            flags: MAP_PRIVATE | MAP_ANONYMOUS,
            offset: 0,
            name: name.map(|s| s.to_string()),
        }
    }

    pub fn is_readable(&self)   -> bool { self.prot & PROT_READ  != 0 }
    pub fn is_writable(&self)   -> bool { self.prot & PROT_WRITE != 0 }
    pub fn is_executable(&self) -> bool { self.prot & PROT_EXEC  != 0 }

    /// Produce the prot string used in /proc/self/maps (e.g. "r-xp").
    fn prot_str(&self) -> [u8; 4] {
        [
            if self.is_readable()   { b'r' } else { b'-' },
            if self.is_writable()   { b'w' } else { b'-' },
            if self.is_executable() { b'x' } else { b'-' },
            if self.flags & MAP_SHARED != 0 { b's' } else { b'p' },
        ]
    }
}

/// Per-process VMA map.  Keys are VMA start addresses for O(log n) lookup.
#[derive(Clone)]
pub struct Vmm {
    vmas: BTreeMap<u64, Vma>,
}

impl Vmm {
    pub const fn new() -> Self {
        Self { vmas: BTreeMap::new() }
    }

    /// Insert (or replace) a VMA.  Splits any existing VMA that partially
    /// overlaps the new range.
    pub fn map(&mut self, start: u64, end: u64, prot: u32, flags: u32,
               offset: u64, name: Option<String>) {
        // Remove any VMAs that are fully covered by [start, end)
        self.remove_range(start, end);
        self.vmas.insert(start, Vma { start, end, prot, flags, offset, name });
    }

    /// Convenience wrapper for anonymous mappings.
    pub fn map_anon(&mut self, start: u64, len: u64, prot: u32, name: Option<&str>) {
        self.map(start, start + len, prot, MAP_PRIVATE | MAP_ANONYMOUS, 0,
                 name.map(|s| s.to_string()));
    }

    /// Remove all VMAs (or portions of VMAs) that overlap [start, start+len).
    pub fn unmap(&mut self, start: u64, len: u64) {
        self.remove_range(start, start + len);
    }

    /// Update protection flags for all pages in [start, start+len).
    pub fn mprotect(&mut self, start: u64, len: u64, prot: u32) {
        let end = start + len;
        // Collect keys of VMAs that overlap
        let keys: Vec<u64> = self.vmas.range(..end)
            .filter(|(_, v)| v.end > start)
            .map(|(k, _)| *k)
            .collect();

        for key in keys {
            if let Some(mut vma) = self.vmas.remove(&key) {
                // Trim / split if needed, then set prot
                if vma.start < start {
                    // Left tail: keep original prot
                    let left = Vma { end: start, ..vma.clone() };
                    self.vmas.insert(left.start, left);
                    vma.start = start;
                }
                if vma.end > end {
                    // Right tail: keep original prot
                    let mut right = vma.clone();
                    right.start = end;
                    self.vmas.insert(right.start, right);
                    vma.end = end;
                }
                vma.prot = prot;
                self.vmas.insert(vma.start, vma);
            }
        }
    }

    /// Find the VMA that contains `addr`, if any.
    pub fn find(&self, addr: u64) -> Option<&Vma> {
        // The largest key ≤ addr
        self.vmas.range(..=addr).next_back().and_then(|(_, v)| {
            if v.end > addr { Some(v) } else { None }
        })
    }

    /// Return the number of mapped VMAs.
    pub fn len(&self) -> usize { self.vmas.len() }

    /// Return a stable snapshot of all VMAs ordered by start address.
    pub fn snapshot(&self) -> Vec<Vma> {
        self.vmas.values().cloned().collect()
    }

    /// Render /proc/self/maps content.
    pub fn maps_string(&self) -> String {
        let mut out = String::new();
        for vma in self.vmas.values() {
            let ps = vma.prot_str();
            let prot_s = core::str::from_utf8(&ps).unwrap_or("----");
            let name_s = vma.name.as_deref().unwrap_or("");
            // Format: start-end perm offset 00:00 0  name
            out.push_str(&alloc::format!(
                "{:016x}-{:016x} {} {:08x} 00:00 0          {}\n",
                vma.start, vma.end, prot_s, vma.offset, name_s
            ));
        }
        out
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    fn remove_range(&mut self, start: u64, end: u64) {
        // Collect fully or partially overlapping entries
        let keys: Vec<u64> = self.vmas.range(..end)
            .filter(|(_, v)| v.end > start)
            .map(|(k, _)| *k)
            .collect();

        for key in keys {
            if let Some(vma) = self.vmas.remove(&key) {
                // Keep left remnant
                if vma.start < start {
                    let mut left = vma.clone();
                    left.end = start;
                    self.vmas.insert(left.start, left);
                }
                // Keep right remnant
                if vma.end > end {
                    let mut right = vma;
                    right.start = end;
                    self.vmas.insert(right.start, right);
                }
            }
        }
    }
}
