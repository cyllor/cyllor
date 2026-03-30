// VirtIO Block Device driver
// Supports read/write via virtqueue 0

use super::mmio::VirtioMmio;
use crate::mm::pmm;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering, fence};
use spin::Mutex;

const SECTOR_SIZE: usize = 512;
const QUEUE_SIZE: u16 = 16;

// VirtIO block request types
const VIRTIO_BLK_T_IN: u32 = 0;  // Read
const VIRTIO_BLK_T_OUT: u32 = 1; // Write

// Descriptor flags
const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;

// Block request header
#[repr(C)]
struct VirtioBlkReqHeader {
    req_type: u32,
    reserved: u32,
    sector: u64,
}

// Virtqueue descriptor
#[repr(C)]
#[derive(Clone, Copy)]
struct VringDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

// Available ring
#[repr(C)]
struct VringAvail {
    flags: u16,
    idx: u16,
    ring: [u16; QUEUE_SIZE as usize],
}

// Used ring element
#[repr(C)]
#[derive(Clone, Copy)]
struct VringUsedElem {
    id: u32,
    len: u32,
}

// Used ring
#[repr(C)]
struct VringUsed {
    flags: u16,
    idx: u16,
    ring: [VringUsedElem; QUEUE_SIZE as usize],
}

pub struct VirtioBlock {
    mmio: VirtioMmio,
    capacity: u64, // in sectors
    // Virtqueue pointers (physical addresses accessible via HHDM)
    desc_ptr: *mut VringDesc,
    avail_ptr: *mut VringAvail,
    used_ptr: *mut VringUsed,
    // Request buffers
    req_header_phys: u64,
    req_header_ptr: *mut VirtioBlkReqHeader,
    data_buf_phys: u64,
    data_buf_ptr: *mut u8,
    status_phys: u64,
    status_ptr: *mut u8,
    // Tracking
    last_used_idx: u16,
    hhdm: u64,
}

unsafe impl Send for VirtioBlock {}
unsafe impl Sync for VirtioBlock {}

static BLOCK_DEV: Mutex<Option<VirtioBlock>> = Mutex::new(None);

impl VirtioBlock {
    fn phys_to_virt(&self, phys: u64) -> u64 {
        phys + self.hhdm
    }
}

/// Probe for VirtIO block devices on QEMU virt machine
/// QEMU virt puts VirtIO MMIO devices at 0x0a000000 + n*0x200
pub fn probe(hhdm: u64) {
    for i in 0..32 {
        let base = 0x0a000000 + i * 0x200 + hhdm as usize;
        if let Some(mmio) = VirtioMmio::new(base) {
            let device_id = mmio.device_id();
            if device_id == 0 {
                continue; // No device
            }
            log::debug!("VirtIO device at 0x{:x}: id={}", base, device_id);
            if device_id == 2 {
                // Block device
                init_block_device(mmio, hhdm);
                return;
            }
        }
    }
    log::info!("No VirtIO block device found");
}

fn init_block_device(mmio: VirtioMmio, hhdm: u64) {
    if !mmio.init() {
        log::error!("VirtIO block init failed");
        return;
    }

    // Read capacity from config
    let capacity: u64 = mmio.read_config(0);
    log::info!("VirtIO block: {} sectors ({} MiB)", capacity, capacity * 512 / (1024 * 1024));

    // Allocate virtqueue memory
    let desc_phys = pmm::alloc_page().unwrap() as u64;
    let avail_phys = pmm::alloc_page().unwrap() as u64;
    let used_phys = pmm::alloc_page().unwrap() as u64;

    // Zero the pages
    unsafe {
        ptr::write_bytes((desc_phys + hhdm) as *mut u8, 0, 4096);
        ptr::write_bytes((avail_phys + hhdm) as *mut u8, 0, 4096);
        ptr::write_bytes((used_phys + hhdm) as *mut u8, 0, 4096);
    }

    // Set up virtqueue
    mmio.setup_queue(0, QUEUE_SIZE, desc_phys, avail_phys, used_phys);

    // Allocate request buffers
    let req_page_phys = pmm::alloc_page().unwrap() as u64;
    unsafe { ptr::write_bytes((req_page_phys + hhdm) as *mut u8, 0, 4096); }

    let data_page_phys = pmm::alloc_page().unwrap() as u64;
    unsafe { ptr::write_bytes((data_page_phys + hhdm) as *mut u8, 0, 4096); }

    // Mark driver as OK
    mmio.driver_ok();

    let dev = VirtioBlock {
        mmio,
        capacity,
        desc_ptr: (desc_phys + hhdm) as *mut VringDesc,
        avail_ptr: (avail_phys + hhdm) as *mut VringAvail,
        used_ptr: (used_phys + hhdm) as *mut VringUsed,
        req_header_phys: req_page_phys,
        req_header_ptr: (req_page_phys + hhdm) as *mut VirtioBlkReqHeader,
        data_buf_phys: data_page_phys,
        data_buf_ptr: (data_page_phys + hhdm) as *mut u8,
        status_phys: req_page_phys + 16, // status byte after header
        status_ptr: (req_page_phys + hhdm + 16) as *mut u8,
        last_used_idx: 0,
        hhdm,
    };

    *BLOCK_DEV.lock() = Some(dev);
    log::info!("VirtIO block device initialized");
}

/// Read sectors from disk
pub fn read_sectors(start_sector: u64, count: usize, buf: &mut [u8]) -> Result<(), &'static str> {
    let mut dev = BLOCK_DEV.lock();
    let dev = dev.as_mut().ok_or("No block device")?;

    let mut offset = 0;
    for s in 0..count {
        let sector = start_sector + s as u64;

        // Set up request header
        unsafe {
            (*dev.req_header_ptr).req_type = VIRTIO_BLK_T_IN;
            (*dev.req_header_ptr).reserved = 0;
            (*dev.req_header_ptr).sector = sector;
            *dev.status_ptr = 0xFF; // Will be overwritten by device
        }

        // Set up descriptor chain: header -> data -> status
        let descs = dev.desc_ptr;
        unsafe {
            // Descriptor 0: request header (device reads)
            (*descs.add(0)).addr = dev.req_header_phys;
            (*descs.add(0)).len = 16;
            (*descs.add(0)).flags = VRING_DESC_F_NEXT;
            (*descs.add(0)).next = 1;

            // Descriptor 1: data buffer (device writes)
            (*descs.add(1)).addr = dev.data_buf_phys;
            (*descs.add(1)).len = SECTOR_SIZE as u32;
            (*descs.add(1)).flags = VRING_DESC_F_WRITE | VRING_DESC_F_NEXT;
            (*descs.add(1)).next = 2;

            // Descriptor 2: status byte (device writes)
            (*descs.add(2)).addr = dev.status_phys;
            (*descs.add(2)).len = 1;
            (*descs.add(2)).flags = VRING_DESC_F_WRITE;
            (*descs.add(2)).next = 0;
        }

        // Add to available ring
        let avail = dev.avail_ptr;
        unsafe {
            let idx = (*avail).idx;
            (*avail).ring[(idx % QUEUE_SIZE) as usize] = 0; // Descriptor chain starts at 0
            fence(Ordering::Release);
            (*avail).idx = idx.wrapping_add(1);
        }

        // Notify device
        fence(Ordering::SeqCst);
        dev.mmio.notify(0);

        // Wait for completion (poll used ring)
        let used = dev.used_ptr;
        loop {
            fence(Ordering::Acquire);
            let used_idx = unsafe { ptr::read_volatile(&(*used).idx) };
            if used_idx != dev.last_used_idx {
                break;
            }
            core::hint::spin_loop();
        }
        dev.last_used_idx = dev.last_used_idx.wrapping_add(1);
        dev.mmio.ack_interrupt();

        // Check status
        let status = unsafe { *dev.status_ptr };
        if status != 0 {
            return Err("Block read failed");
        }

        // Copy data to output buffer
        let copy_len = SECTOR_SIZE.min(buf.len() - offset);
        unsafe {
            ptr::copy_nonoverlapping(dev.data_buf_ptr, buf[offset..].as_mut_ptr(), copy_len);
        }
        offset += copy_len;
    }

    Ok(())
}

/// Write sectors to disk
pub fn write_sectors(start_sector: u64, count: usize, data: &[u8]) -> Result<(), &'static str> {
    let mut dev = BLOCK_DEV.lock();
    let dev = dev.as_mut().ok_or("No block device")?;

    let mut offset = 0;
    for s in 0..count {
        let sector = start_sector + s as u64;
        let copy_len = SECTOR_SIZE.min(data.len() - offset);

        // Copy data to device buffer
        unsafe {
            ptr::copy_nonoverlapping(data[offset..].as_ptr(), dev.data_buf_ptr, copy_len);
        }

        // Set up request header
        unsafe {
            (*dev.req_header_ptr).req_type = VIRTIO_BLK_T_OUT;
            (*dev.req_header_ptr).reserved = 0;
            (*dev.req_header_ptr).sector = sector;
            *dev.status_ptr = 0xFF;
        }

        let descs = dev.desc_ptr;
        unsafe {
            (*descs.add(0)).addr = dev.req_header_phys;
            (*descs.add(0)).len = 16;
            (*descs.add(0)).flags = VRING_DESC_F_NEXT;
            (*descs.add(0)).next = 1;

            (*descs.add(1)).addr = dev.data_buf_phys;
            (*descs.add(1)).len = SECTOR_SIZE as u32;
            (*descs.add(1)).flags = VRING_DESC_F_NEXT;
            (*descs.add(1)).next = 2;

            (*descs.add(2)).addr = dev.status_phys;
            (*descs.add(2)).len = 1;
            (*descs.add(2)).flags = VRING_DESC_F_WRITE;
            (*descs.add(2)).next = 0;
        }

        let avail = dev.avail_ptr;
        unsafe {
            let idx = (*avail).idx;
            (*avail).ring[(idx % QUEUE_SIZE) as usize] = 0;
            fence(Ordering::Release);
            (*avail).idx = idx.wrapping_add(1);
        }

        fence(Ordering::SeqCst);
        dev.mmio.notify(0);

        let used = dev.used_ptr;
        loop {
            fence(Ordering::Acquire);
            let used_idx = unsafe { ptr::read_volatile(&(*used).idx) };
            if used_idx != dev.last_used_idx {
                break;
            }
            core::hint::spin_loop();
        }
        dev.last_used_idx = dev.last_used_idx.wrapping_add(1);
        dev.mmio.ack_interrupt();

        let status = unsafe { *dev.status_ptr };
        if status != 0 {
            return Err("Block write failed");
        }

        offset += copy_len;
    }

    Ok(())
}

pub fn capacity_sectors() -> u64 {
    BLOCK_DEV.lock().as_ref().map(|d| d.capacity).unwrap_or(0)
}
