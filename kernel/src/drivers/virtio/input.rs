// virtio-input driver (device ID 18)
// Reads keyboard/mouse events from QEMU via the event queue (queue 0).

use super::mmio::VirtioMmio;
use spin::Mutex;

const QUEUE_SIZE: u16 = 64;
const EVENT_SIZE: usize = 8; // sizeof(virtio_input_event)

// virtio_input_event: { ev_type: u16, code: u16, value: u32 }
#[repr(C)]
#[derive(Clone, Copy)]
struct InputEvent {
    ev_type: u16,
    code: u16,
    value: u32,
}

// Virtqueue structures (same layout as block driver)
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

struct VirtioInput {
    mmio: VirtioMmio,
    // MMIO slot index (0-based), determines which IRQ fires for this device
    slot: usize,
    // Event queue pointers (virtual addresses in HHDM)
    desc_ptr: *mut VringDesc,
    avail_ptr: *mut VringAvail,
    used_ptr: *mut VringUsed,
    // Pre-allocated event buffers (QUEUE_SIZE buffers of EVENT_SIZE each)
    event_bufs_ptr: *mut u8,
    last_used_idx: u16,
}

unsafe impl Send for VirtioInput {}
unsafe impl Sync for VirtioInput {}

static DEVICES: Mutex<alloc::vec::Vec<VirtioInput>> = Mutex::new(alloc::vec::Vec::new());

/// Probe a single virtio MMIO slot for a virtio-input device (device ID 18).
/// `base` is the physical base address of the slot.
/// `slot` is the index of this slot (0-based, matches IRQ offset from 48).
/// `hhdm` is the higher-half direct mapping offset.
/// Returns true if a virtio-input device was found and initialised.
pub fn probe(base: usize, slot: usize, hhdm: u64) -> bool {
    let mmio = match VirtioMmio::new(base + hhdm as usize) {
        Some(m) => m,
        None => return false,
    };

    if mmio.device_id() != 18 {
        return false;
    }

    log::info!("Found virtio-input at 0x{:x} (slot {})", base, slot);

    if !mmio.init() {
        log::error!("virtio-input init failed");
        return false;
    }

    // --- Virtqueue memory layout ---
    // Descriptors: sizeof(VringDesc) * QUEUE_SIZE
    // Available ring: 4 bytes header + 2 * QUEUE_SIZE bytes
    // Used ring: must be 4096-aligned; 4 bytes header + 8 * QUEUE_SIZE bytes
    let desc_size  = core::mem::size_of::<VringDesc>() * QUEUE_SIZE as usize; // 16 * 64 = 1024
    let avail_size = 4 + 2 * QUEUE_SIZE as usize;                              // 4 + 128 = 132

    // Allocate one allocation large enough for desc + avail + padding + used, plus one extra
    // page for alignment.  We derive physical addresses via HHDM (heap is in HHDM-mapped space).
    let used_size   = 4 + 8 * QUEUE_SIZE as usize; // 4 + 512 = 516
    let alloc_bytes = desc_size + avail_size + 4096 /* pad to align used */ + used_size + 4096 /* head alignment */;

    let queue_buf: alloc::boxed::Box<[u8]> = alloc::vec![0u8; alloc_bytes].into_boxed_slice();
    let queue_virt_raw = queue_buf.as_ptr() as u64;
    // Align start to 4096
    let queue_virt = (queue_virt_raw + 4095) & !4095;
    let queue_phys = queue_virt - hhdm;
    // Leak the allocation — the driver owns it for the kernel lifetime
    core::mem::forget(queue_buf);

    let desc_phys  = queue_phys;
    let avail_phys = desc_phys + desc_size as u64;
    let used_phys  = (avail_phys + avail_size as u64 + 4095) & !4095;

    let desc_ptr  = queue_virt as *mut VringDesc;
    let avail_ptr = (queue_virt + desc_size as u64) as *mut VringAvail;
    let used_ptr  = ((queue_virt + desc_size as u64 + avail_size as u64 + 4095) & !4095) as *mut VringUsed;

    // --- Event buffers ---
    let event_bufs_size = EVENT_SIZE * QUEUE_SIZE as usize; // 8 * 64 = 512
    let event_buf: alloc::boxed::Box<[u8]> = alloc::vec![0u8; event_bufs_size + 4096].into_boxed_slice();
    let event_bufs_virt = (event_buf.as_ptr() as u64 + 4095) & !4095;
    let event_bufs_phys = event_bufs_virt - hhdm;
    core::mem::forget(event_buf);

    // --- Fill descriptor table ---
    // Each descriptor points to one 8-byte event buffer; VRING_DESC_F_WRITE (2) = device-writable
    for i in 0..QUEUE_SIZE as usize {
        unsafe {
            (*desc_ptr.add(i)) = VringDesc {
                addr:  event_bufs_phys + (i * EVENT_SIZE) as u64,
                len:   EVENT_SIZE as u32,
                flags: 2, // VRING_DESC_F_WRITE
                next:  0,
            };
        }
    }

    // --- Fill available ring: offer all descriptors to the device ---
    unsafe {
        (*avail_ptr).flags = 0;
        (*avail_ptr).idx   = QUEUE_SIZE;
        for i in 0..QUEUE_SIZE as usize {
            (*avail_ptr).ring[i] = i as u16;
        }
    }

    // --- Register the queue with the device ---
    mmio.setup_queue(0, QUEUE_SIZE, desc_phys, avail_phys, used_phys);
    mmio.driver_ok();

    // Enable the specific GIC IRQ for this slot (INTID 48 + slot)
    crate::arch::enable_irq(48 + slot as u32);

    let dev = VirtioInput {
        mmio,
        slot,
        desc_ptr,
        avail_ptr,
        used_ptr,
        event_bufs_ptr: event_bufs_virt as *mut u8,
        last_used_idx: 0,
    };

    DEVICES.lock().push(dev);
    true
}

/// Called from the IRQ handler when a virtio-input device fires.
/// `irq_slot` = IRQ number − 48 = the MMIO slot index.
pub fn handle_irq(irq_slot: usize) {
    let mut devices = DEVICES.lock();
    let dev = match devices.iter_mut().find(|d| d.slot == irq_slot) {
        Some(d) => d,
        None => return,
    };

    dev.mmio.ack_interrupt();

    // Drain completed entries from the used ring
    loop {
        let used_idx = unsafe { core::ptr::read_volatile(&(*dev.used_ptr).idx) };
        if dev.last_used_idx == used_idx { break; }

        let elem_pos = (dev.last_used_idx % QUEUE_SIZE) as usize;
        let elem = unsafe { core::ptr::read_volatile(&(*dev.used_ptr).ring[elem_pos]) };
        dev.last_used_idx = dev.last_used_idx.wrapping_add(1);

        // Read the event from the pre-allocated buffer identified by descriptor index
        let buf_ptr = unsafe { dev.event_bufs_ptr.add(elem.id as usize * EVENT_SIZE) };
        let ev = unsafe { core::ptr::read_volatile(buf_ptr as *const InputEvent) };

        // Forward to the input subsystem
        crate::drivers::input::push_virtio_event(ev.ev_type, ev.code, ev.value);

        // Re-offer the descriptor to the device
        let desc_idx = elem.id as u16;
        unsafe {
            let avail_idx = core::ptr::read_volatile(&(*dev.avail_ptr).idx);
            let ring_pos  = (avail_idx % QUEUE_SIZE) as usize;
            core::ptr::write_volatile(&mut (*dev.avail_ptr).ring[ring_pos], desc_idx);
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            core::ptr::write_volatile(&mut (*dev.avail_ptr).idx, avail_idx.wrapping_add(1));
        }

        dev.mmio.notify(0);
    }
}

/// Polling fallback — call from timer tick if interrupts are not yet wired.
pub fn poll_all() {
    let count = DEVICES.lock().len();
    for i in 0..count {
        // Grab each device's slot, then call handle_irq
        let slot = DEVICES.lock().get(i).map(|d| d.slot);
        if let Some(s) = slot {
            handle_irq(s);
        }
    }
}
