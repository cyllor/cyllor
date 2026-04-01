use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::syscall::{SyscallResult, EINVAL, ENOENT, ENOMEM, EEXIST};

struct ShmSegment {
    key: i32,
    size: usize,
    data: Arc<Mutex<Vec<u8>>>,
    id: i32,
}

static SHM_SEGS: Mutex<BTreeMap<i32, ShmSegment>> = Mutex::new(BTreeMap::new());
static NEXT_SHMID: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(1);

// IPC_PRIVATE = 0
// IPC_CREAT = 0o1000, IPC_EXCL = 0o2000

pub fn do_shmget(key: i32, size: usize, shmflg: i32) -> SyscallResult {
    const IPC_PRIVATE: i32 = 0;
    const IPC_CREAT: i32 = 0o1000;
    const IPC_EXCL: i32 = 0o2000;

    if key == IPC_PRIVATE || shmflg & IPC_CREAT != 0 {
        // Create new segment
        let id = NEXT_SHMID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let seg = ShmSegment {
            key,
            size,
            data: Arc::new(Mutex::new(alloc::vec![0u8; size])),
            id,
        };
        SHM_SEGS.lock().insert(id, seg);
        Ok(id as usize)
    } else {
        // Look up existing
        let segs = SHM_SEGS.lock();
        for seg in segs.values() {
            if seg.key == key {
                return Ok(seg.id as usize);
            }
        }
        Err(ENOENT)
    }
}

pub fn do_shmat(shmid: i32, shmaddr: u64, shmflg: i32) -> SyscallResult {
    let data_info = {
        let segs = SHM_SEGS.lock();
        segs.get(&shmid).map(|s| {
            let data = s.data.lock();
            let ptr = data.as_ptr() as u64;
            (ptr, s.size)
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

    let flags = crate::mm::page_table::PageFlags::USER_RW;
    let pages = (size + 4095) / 4096;
    for i in 0..pages {
        let page_virt = virt_addr + (i * 4096) as u64;
        // page_virt is in kernel heap (HHDM mapped), get phys
        let phys = page_virt - hhdm;
        crate::arch::map_user_page(l0_phys, (map_addr + i * 4096) as u64, phys, flags);
    }

    Ok(map_addr)
}

pub fn do_shmdt(shmaddr: u64) -> SyscallResult {
    Ok(0) // mapping stays valid; Linux allows access after shmdt until next exec
}

/// Remove all SHM segments whose data Arc has no external holders (called on process exit).
/// A segment with strong_count == 1 means only SHM_SEGS holds the Arc — no process is attached.
pub fn cleanup_orphaned() {
    SHM_SEGS.lock().retain(|_, seg| {
        Arc::strong_count(&seg.data) > 1
    });
}

pub fn do_shmctl(shmid: i32, cmd: i32, buf: u64) -> SyscallResult {
    const IPC_RMID: i32 = 0;
    const IPC_SET: i32 = 1;
    const IPC_STAT: i32 = 2;

    match cmd {
        IPC_RMID => {
            SHM_SEGS.lock().remove(&shmid);
            Ok(0)
        }
        IPC_STAT => {
            // Write shmid_ds struct (minimal)
            if buf != 0 {
                unsafe { core::ptr::write_bytes(buf as *mut u8, 0, 112); }
                if let Some(seg) = SHM_SEGS.lock().get(&shmid) {
                    unsafe { *((buf + 48) as *mut u64) = seg.size as u64; } // shm_segsz
                }
            }
            Ok(0)
        }
        _ => Ok(0),
    }
}
