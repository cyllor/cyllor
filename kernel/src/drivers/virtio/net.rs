// VirtIO Network Device driver (device ID = 1)
// Queues: 0 = RX (device→driver), 1 = TX (driver→device)
// Each frame is prefixed by a 10-byte VirtioNetHdr.

use super::mmio::VirtioMmio;
use crate::mm::pmm;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::ptr;
use core::sync::atomic::{fence, Ordering};
use spin::Mutex;

const QUEUE_SIZE: u16 = 64;
const NET_HDR_LEN: usize = 10; // VirtioNetHdr without num_buffers

// Descriptor flags
const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioNetHdr {
    flags: u8,
    gso_type: u8,
    hdr_len: u16,
    gso_size: u16,
    csum_start: u16,
    csum_offset: u16,
    // num_buffers omitted (no VIRTIO_NET_F_MRG_RXBUF)
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VringDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
struct VringAvail {
    flags: u16,
    idx: u16,
    ring: [u16; QUEUE_SIZE as usize],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VringUsedElem {
    id: u32,
    len: u32,
}

#[repr(C)]
struct VringUsed {
    flags: u16,
    idx: u16,
    ring: [VringUsedElem; QUEUE_SIZE as usize],
}

pub struct VirtioNet {
    mmio: VirtioMmio,
    hhdm: u64,
    // RX queue (index 0)
    rx_desc: *mut VringDesc,
    rx_avail: *mut VringAvail,
    rx_used: *mut VringUsed,
    rx_bufs: [u64; QUEUE_SIZE as usize], // physical addresses of RX buffers
    rx_last_used: u16,
    // TX queue (index 1)
    tx_desc: *mut VringDesc,
    tx_avail: *mut VringAvail,
    tx_used: *mut VringUsed,
    tx_buf_phys: u64, // one reusable TX buffer (hdr + frame, up to 1526 bytes)
    tx_last_used: u16,
    tx_avail_idx: u16,
    // Received frames waiting to be consumed by smoltcp
    pub rx_pending: VecDeque<Vec<u8>>,
    // MAC address
    pub mac: [u8; 6],
}

unsafe impl Send for VirtioNet {}
unsafe impl Sync for VirtioNet {}

static NET_DEV: Mutex<Option<VirtioNet>> = Mutex::new(None);

/// Probe all VirtIO MMIO slots for a net device (device ID = 1).
pub fn probe(hhdm: u64) {
    let mmio_base = crate::arch::VIRTIO_MMIO_BASE;
    if mmio_base == 0 { return; }
    for i in 0..32 {
        let base = mmio_base + i * crate::arch::VIRTIO_MMIO_STRIDE + hhdm as usize;
        if let Some(mmio) = VirtioMmio::new(base) {
            if mmio.device_id() == 1 {
                log::info!("VirtIO net device found at 0x{:x}", base - hhdm as usize);
                init_net_device(mmio, hhdm);
                return;
            }
        }
    }
    log::info!("No VirtIO net device found");
}

fn alloc_queue(hhdm: u64) -> (u64, u64, u64, *mut VringDesc, *mut VringAvail, *mut VringUsed) {
    // Two pages: first holds desc+avail, second holds used (must be 4096-aligned)
    let p0 = pmm::alloc_page().unwrap() as u64;
    let p1 = pmm::alloc_page().unwrap() as u64;
    unsafe {
        ptr::write_bytes((p0 + hhdm) as *mut u8, 0, 4096);
        ptr::write_bytes((p1 + hhdm) as *mut u8, 0, 4096);
    }
    let desc_phys = p0;
    let avail_phys = p0 + core::mem::size_of::<VringDesc>() as u64 * QUEUE_SIZE as u64;
    let used_phys = p1;
    let desc = (desc_phys + hhdm) as *mut VringDesc;
    let avail = (avail_phys + hhdm) as *mut VringAvail;
    let used = (used_phys + hhdm) as *mut VringUsed;
    (desc_phys, avail_phys, used_phys, desc, avail, used)
}

fn init_net_device(mmio: VirtioMmio, hhdm: u64) {
    if !mmio.init() {
        log::error!("VirtIO net init failed");
        return;
    }

    // Read MAC from config (bytes 0-5 in the config space)
    let mac = [
        mmio.read_config::<u8>(0),
        mmio.read_config::<u8>(1),
        mmio.read_config::<u8>(2),
        mmio.read_config::<u8>(3),
        mmio.read_config::<u8>(4),
        mmio.read_config::<u8>(5),
    ];
    log::info!("VirtIO net MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

    // Set up RX queue (0)
    let (rx_desc_phys, rx_avail_phys, rx_used_phys, rx_desc, rx_avail, rx_used) =
        alloc_queue(hhdm);
    mmio.setup_queue(0, QUEUE_SIZE, rx_desc_phys, rx_avail_phys, rx_used_phys);

    // Set up TX queue (1)
    let (tx_desc_phys, tx_avail_phys, tx_used_phys, tx_desc, tx_avail, tx_used) =
        alloc_queue(hhdm);
    mmio.setup_queue(1, QUEUE_SIZE, tx_desc_phys, tx_avail_phys, tx_used_phys);

    mmio.driver_ok();

    // Allocate RX buffers: one page per slot, pre-populate RX queue
    let mut rx_bufs = [0u64; QUEUE_SIZE as usize];
    for i in 0..QUEUE_SIZE as usize {
        let buf_phys = pmm::alloc_page().unwrap() as u64;
        unsafe { ptr::write_bytes((buf_phys + hhdm) as *mut u8, 0, 4096); }
        rx_bufs[i] = buf_phys;
        // Descriptor: device-writable, no chain
        unsafe {
            (*rx_desc.add(i)).addr = buf_phys;
            (*rx_desc.add(i)).len = 4096;
            (*rx_desc.add(i)).flags = VRING_DESC_F_WRITE;
            (*rx_desc.add(i)).next = 0;
            // Add to avail ring
            (*rx_avail).ring[i] = i as u16;
        }
    }
    unsafe {
        fence(Ordering::Release);
        (*rx_avail).idx = QUEUE_SIZE;
    }
    // Notify device about RX queue
    mmio.notify(0);

    // TX buffer: one page for hdr + frame
    let tx_buf_phys = pmm::alloc_page().unwrap() as u64;
    unsafe { ptr::write_bytes((tx_buf_phys + hhdm) as *mut u8, 0, 4096); }

    *NET_DEV.lock() = Some(VirtioNet {
        mmio,
        hhdm,
        rx_desc,
        rx_avail,
        rx_used,
        rx_bufs,
        rx_last_used: 0,
        tx_desc,
        tx_avail,
        tx_used,
        tx_buf_phys,
        tx_last_used: 0,
        tx_avail_idx: 0,
        rx_pending: VecDeque::new(),
        mac,
    });
    log::info!("VirtIO net device initialized");
}

/// Transmit a raw Ethernet frame (without VirtioNetHdr — we prepend it here).
pub fn transmit_frame(frame: &[u8]) {
    let mut dev_lock = NET_DEV.lock();
    let dev = match dev_lock.as_mut() {
        Some(d) => d,
        None => return,
    };

    if frame.len() + NET_HDR_LEN > 4096 {
        return;
    }

    let hhdm = dev.hhdm;
    let buf_virt = (dev.tx_buf_phys + hhdm) as *mut u8;

    // Write net header (all-zero = no offload)
    unsafe { ptr::write_bytes(buf_virt, 0, NET_HDR_LEN); }
    // Write frame after header
    unsafe { ptr::copy_nonoverlapping(frame.as_ptr(), buf_virt.add(NET_HDR_LEN), frame.len()); }

    let total = NET_HDR_LEN + frame.len();
    let desc_idx = (dev.tx_avail_idx % QUEUE_SIZE) as usize;

    unsafe {
        (*dev.tx_desc.add(desc_idx)).addr = dev.tx_buf_phys;
        (*dev.tx_desc.add(desc_idx)).len = total as u32;
        (*dev.tx_desc.add(desc_idx)).flags = 0; // device reads
        (*dev.tx_desc.add(desc_idx)).next = 0;

        let avail_idx = (*dev.tx_avail).idx;
        (*dev.tx_avail).ring[(avail_idx % QUEUE_SIZE) as usize] = desc_idx as u16;
        fence(Ordering::Release);
        (*dev.tx_avail).idx = avail_idx.wrapping_add(1);
    }

    fence(Ordering::SeqCst);
    dev.mmio.notify(1);
    dev.tx_avail_idx = dev.tx_avail_idx.wrapping_add(1);

    // Drain used ring so we don't stall (we reuse the single TX buffer)
    let used = dev.tx_used;
    for _ in 0..64 {
        fence(Ordering::Acquire);
        let used_idx = unsafe { ptr::read_volatile(&(*used).idx) };
        if used_idx == dev.tx_last_used { break; }
        dev.tx_last_used = dev.tx_last_used.wrapping_add(1);
    }
}

/// Poll the RX queue and collect any received frames (stripping VirtioNetHdr).
/// Returns newly received frames, pushed into the device's `rx_pending`.
pub fn poll_rx() {
    let mut dev_lock = NET_DEV.lock();
    let dev = match dev_lock.as_mut() {
        Some(d) => d,
        None => return,
    };

    let hhdm = dev.hhdm;
    let used = dev.rx_used;
    loop {
        fence(Ordering::Acquire);
        let used_idx = unsafe { ptr::read_volatile(&(*used).idx) };
        if used_idx == dev.rx_last_used { break; }

        let elem = unsafe { ptr::read_volatile(&(*used).ring[(dev.rx_last_used % QUEUE_SIZE) as usize]) };
        let desc_idx = elem.id as usize;
        let total_len = elem.len as usize;

        if total_len > NET_HDR_LEN {
            let frame_len = total_len - NET_HDR_LEN;
            let buf_virt = (dev.rx_bufs[desc_idx] + hhdm) as *const u8;
            let frame_data = unsafe {
                core::slice::from_raw_parts(buf_virt.add(NET_HDR_LEN), frame_len)
            };
            dev.rx_pending.push_back(frame_data.to_vec());
        }

        // Return descriptor to available ring
        let rx_desc = dev.rx_desc;
        let rx_avail = dev.rx_avail;
        unsafe {
            (*rx_desc.add(desc_idx)).addr = dev.rx_bufs[desc_idx];
            (*rx_desc.add(desc_idx)).len = 4096;
            (*rx_desc.add(desc_idx)).flags = VRING_DESC_F_WRITE;
            (*rx_desc.add(desc_idx)).next = 0;

            let avail_idx = (*rx_avail).idx;
            (*rx_avail).ring[(avail_idx % QUEUE_SIZE) as usize] = desc_idx as u16;
            fence(Ordering::Release);
            (*rx_avail).idx = avail_idx.wrapping_add(1);
        }
        dev.mmio.notify(0);
        dev.rx_last_used = dev.rx_last_used.wrapping_add(1);
    }
}

/// Take all pending received frames.
pub fn take_rx_frames() -> Vec<Vec<u8>> {
    let mut dev_lock = NET_DEV.lock();
    match dev_lock.as_mut() {
        Some(d) => d.rx_pending.drain(..).collect(),
        None => Vec::new(),
    }
}

/// Return MAC address, or zeroed if no device.
pub fn mac_address() -> [u8; 6] {
    NET_DEV.lock().as_ref().map(|d| d.mac).unwrap_or([0u8; 6])
}

pub fn is_available() -> bool {
    NET_DEV.lock().is_some()
}
