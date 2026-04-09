use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::syscall::{SyscallResult, EINVAL, ENOENT, ENOMEM, EEXIST};
use crate::arch::PageAttr;
use crate::mm::vmm::{MAP_SHARED, PROT_READ, PROT_WRITE};

struct ShmSegment {
    key: i32,
    size: usize,
    data: Arc<Mutex<Vec<u8>>>,
    id: i32,
    attach_count: usize,
    marked_for_delete: bool,
}

static SHM_SEGS: Mutex<BTreeMap<i32, ShmSegment>> = Mutex::new(BTreeMap::new());
static SHM_ATTACH: Mutex<BTreeMap<u64, Vec<(usize, i32, usize)>>> = Mutex::new(BTreeMap::new()); // pid -> [(addr, shmid, size)]
static NEXT_SHMID: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(1);

// IPC_PRIVATE = 0
// IPC_CREAT = 0o1000, IPC_EXCL = 0o2000

pub fn do_shmget(key: i32, size: usize, shmflg: i32) -> SyscallResult {
    const IPC_PRIVATE: i32 = 0;
    const IPC_CREAT: i32 = 0o1000;
    const IPC_EXCL: i32 = 0o2000;

    if size == 0 {
        return Err(EINVAL);
    }

    if key != IPC_PRIVATE {
        let mut existing: Option<i32> = None;
        let segs = SHM_SEGS.lock();
        for seg in segs.values() {
            if seg.key == key && !seg.marked_for_delete {
                existing = Some(seg.id);
                break;
            }
        }
        drop(segs);
        if let Some(id) = existing {
            if (shmflg & IPC_CREAT) != 0 && (shmflg & IPC_EXCL) != 0 {
                return Err(EEXIST);
            }
            return Ok(id as usize);
        }
    }

    if key == IPC_PRIVATE || (shmflg & IPC_CREAT) != 0 {
        // Create new segment
        let id = NEXT_SHMID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let seg = ShmSegment {
            key,
            size,
            data: Arc::new(Mutex::new(alloc::vec![0u8; size])),
            id,
            attach_count: 0,
            marked_for_delete: false,
        };
        SHM_SEGS.lock().insert(id, seg);
        Ok(id as usize)
    } else {
        // Look up existing
        let segs = SHM_SEGS.lock();
        for seg in segs.values() {
            if seg.key == key && !seg.marked_for_delete {
                return Ok(seg.id as usize);
            }
        }
        Err(ENOENT)
    }
}

pub fn do_shmat(shmid: i32, shmaddr: u64, shmflg: i32) -> SyscallResult {
    const SHM_RDONLY: i32 = 0o10000;

    let data_info = {
        let segs = SHM_SEGS.lock();
        segs.get(&shmid).and_then(|s| {
            if s.marked_for_delete {
                return None;
            }
            let data = s.data.lock();
            let ptr = data.as_ptr() as u64;
            Some((ptr, s.size))
        })
    };

    let (virt_addr, size) = match data_info {
        Some(d) => d,
        None => return Err(EINVAL),
    };

    // Get physical address of the shared memory
    let hhdm = crate::arch::hhdm_offset();

    let map_addr = if shmaddr != 0 {
        shmaddr as usize
    } else {
        crate::mm::mmap::shm_alloc_addr(size)
    };

    let l0_phys = crate::arch::read_user_page_table_root();

    let readonly = (shmflg & SHM_RDONLY) != 0;
    let flags = if readonly { PageAttr::USER_RO } else { PageAttr::USER_RW };
    let pages = (size + 4095) / 4096;
    for i in 0..pages {
        let page_virt = virt_addr + (i * 4096) as u64;
        // page_virt is in kernel heap (HHDM mapped), get phys
        let phys = page_virt - hhdm;
        crate::arch::map_user_page(l0_phys, (map_addr + i * 4096) as u64, phys, flags);
    }

    let pid = crate::sched::process::current_pid();
    {
        let mut segs = SHM_SEGS.lock();
        if let Some(seg) = segs.get_mut(&shmid) {
            seg.attach_count = seg.attach_count.saturating_add(1);
        }
    }
    SHM_ATTACH
        .lock()
        .entry(pid)
        .or_insert_with(Vec::new)
        .push((map_addr, shmid, size));
    if let Some(proc) = crate::sched::process::PROCESS_TABLE.lock().get_mut(&pid) {
        proc.vmm.lock().map(
            map_addr as u64,
            (map_addr + size) as u64,
            if readonly { PROT_READ } else { PROT_READ | PROT_WRITE },
            MAP_SHARED,
            0,
            Some(alloc::format!("[shm:{shmid}]")),
        );
    }

    Ok(map_addr)
}

pub fn do_shmdt(shmaddr: u64) -> SyscallResult {
    if shmaddr == 0 || (shmaddr as usize) % 4096 != 0 {
        return Err(EINVAL);
    }

    let pid = crate::sched::process::current_pid();
    let mapping = {
        let mut attach = SHM_ATTACH.lock();
        let entries = attach.get_mut(&pid).ok_or(EINVAL)?;
        let idx = entries
            .iter()
            .position(|(addr, _, _)| *addr == shmaddr as usize)
            .ok_or(EINVAL)?;
        let m = entries.remove(idx);
        if entries.is_empty() {
            attach.remove(&pid);
        }
        m
    };

    let (map_addr, shmid, size) = mapping;
    let root_phys = crate::arch::read_user_page_table_root();
    if root_phys != 0 {
        let pages = (size + 4095) / 4096;
        for i in 0..pages {
            let _ = crate::arch::unmap_user_page(root_phys, (map_addr + i * 4096) as u64);
        }
    }
    if let Some(proc) = crate::sched::process::PROCESS_TABLE.lock().get_mut(&pid) {
        proc.vmm.lock().unmap(map_addr as u64, size as u64);
    }
    {
        let mut segs = SHM_SEGS.lock();
        if let Some(seg) = segs.get_mut(&shmid) {
            seg.attach_count = seg.attach_count.saturating_sub(1);
        }
    }
    cleanup_orphaned();
    Ok(0)
}

/// Remove all SHM segments whose data Arc has no external holders (called on process exit).
/// A segment with strong_count == 1 means only SHM_SEGS holds the Arc — no process is attached.
pub fn cleanup_orphaned() {
    SHM_SEGS
        .lock()
        .retain(|_, seg| !(seg.marked_for_delete && seg.attach_count == 0));
}

pub fn cleanup_process_attachments(pid: u64) {
    let mappings = SHM_ATTACH.lock().remove(&pid).unwrap_or_default();
    if mappings.is_empty() {
        return;
    }
    let mut segs = SHM_SEGS.lock();
    for (_, shmid, _) in mappings {
        if let Some(seg) = segs.get_mut(&shmid) {
            seg.attach_count = seg.attach_count.saturating_sub(1);
        }
    }
    segs.retain(|_, seg| !(seg.marked_for_delete && seg.attach_count == 0));
}

/// Drop SHM attachment bookkeeping for mappings fully covered by [addr, addr+len).
/// Used by munmap() so shm segments are not leaked when userspace unmaps directly.
pub fn detach_mappings_in_range(pid: u64, addr: usize, len: usize) {
    if len == 0 {
        return;
    }
    let end = addr.saturating_add(len);
    let removed = {
        let mut attach = SHM_ATTACH.lock();
        let Some(entries) = attach.get_mut(&pid) else {
            return;
        };

        let mut removed = Vec::new();
        let mut kept = Vec::with_capacity(entries.len());
        for &(maddr, shmid, msize) in entries.iter() {
            let mend = maddr.saturating_add(msize);
            if addr <= maddr && end >= mend {
                removed.push((shmid, msize));
            } else {
                kept.push((maddr, shmid, msize));
            }
        }
        *entries = kept;
        if entries.is_empty() {
            attach.remove(&pid);
        }
        removed
    };

    if removed.is_empty() {
        return;
    }
    let mut segs = SHM_SEGS.lock();
    for (shmid, _) in removed {
        if let Some(seg) = segs.get_mut(&shmid) {
            seg.attach_count = seg.attach_count.saturating_sub(1);
        }
    }
    segs.retain(|_, seg| !(seg.marked_for_delete && seg.attach_count == 0));
}

pub fn do_shmctl(shmid: i32, cmd: i32, buf: u64) -> SyscallResult {
    const IPC_RMID: i32 = 0;
    const IPC_SET: i32 = 1;
    const IPC_STAT: i32 = 2;

    match cmd {
        IPC_RMID => {
            let mut segs = SHM_SEGS.lock();
            if let Some(seg) = segs.get_mut(&shmid) {
                seg.marked_for_delete = true;
            } else {
                return Err(EINVAL);
            }
            segs.retain(|_, seg| !(seg.marked_for_delete && seg.attach_count == 0));
            Ok(0)
        }
        IPC_STAT => {
            // Write shmid_ds struct (minimal)
            if buf != 0 {
                let mut data = [0u8; 112];
                let segs = SHM_SEGS.lock();
                let seg = segs.get(&shmid).ok_or(EINVAL)?;
                data[48..56].copy_from_slice(&(seg.size as u64).to_le_bytes()); // shm_segsz
                crate::syscall::fs::copy_to_user(buf, &data).map_err(|_| crate::syscall::EFAULT)?;
            }
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}
