// VirtIO MMIO transport layer (v2)
// Reference: https://docs.oasis-open.org/virtio/virtio/v1.2/virtio-v1.2.html

use core::ptr;

// MMIO register offsets
const MAGIC: usize = 0x000;
const VERSION: usize = 0x004;
const DEVICE_ID: usize = 0x008;
const VENDOR_ID: usize = 0x00C;
const DEVICE_FEATURES: usize = 0x010;
const DEVICE_FEATURES_SEL: usize = 0x014;
const DRIVER_FEATURES: usize = 0x020;
const DRIVER_FEATURES_SEL: usize = 0x024;
const QUEUE_SEL: usize = 0x030;
const QUEUE_NUM_MAX: usize = 0x034;
const QUEUE_NUM: usize = 0x038;
const QUEUE_READY: usize = 0x044;
const QUEUE_NOTIFY: usize = 0x050;
const INTERRUPT_STATUS: usize = 0x060;
const INTERRUPT_ACK: usize = 0x064;
const STATUS: usize = 0x070;
const QUEUE_DESC_LOW: usize = 0x080;
const QUEUE_DESC_HIGH: usize = 0x084;
const QUEUE_DRIVER_LOW: usize = 0x090;
const QUEUE_DRIVER_HIGH: usize = 0x094;
const QUEUE_DEVICE_LOW: usize = 0x0A0;
const QUEUE_DEVICE_HIGH: usize = 0x0A4;
const CONFIG: usize = 0x100;

// Status bits
const STATUS_ACK: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_FEATURES_OK: u32 = 8;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FAILED: u32 = 128;

const VIRTIO_MAGIC: u32 = 0x74726976; // "virt"

pub struct VirtioMmio {
    base: usize,
}

impl VirtioMmio {
    pub fn new(base: usize) -> Option<Self> {
        let dev = Self { base };
        let magic = dev.read32(MAGIC);
        if magic != VIRTIO_MAGIC {
            return None;
        }
        let version = dev.read32(VERSION);
        if version != 1 && version != 2 {
            return None;
        }
        Some(dev)
    }

    pub fn device_id(&self) -> u32 {
        self.read32(DEVICE_ID)
    }

    pub fn vendor_id(&self) -> u32 {
        self.read32(VENDOR_ID)
    }

    /// Initialize the device following the VirtIO spec
    pub fn init(&self) -> bool {
        // 1. Reset
        self.write32(STATUS, 0);
        // 2. Acknowledge
        self.write32(STATUS, STATUS_ACK);
        // 3. Driver
        self.write32(STATUS, STATUS_ACK | STATUS_DRIVER);
        // 4. Read features, negotiate
        self.write32(DEVICE_FEATURES_SEL, 0);
        let _features = self.read32(DEVICE_FEATURES);
        // Accept all features for now
        self.write32(DRIVER_FEATURES_SEL, 0);
        self.write32(DRIVER_FEATURES, 0); // No features needed
        // 5. Features OK
        self.write32(STATUS, STATUS_ACK | STATUS_DRIVER | STATUS_FEATURES_OK);
        let status = self.read32(STATUS);
        if status & STATUS_FEATURES_OK == 0 {
            self.write32(STATUS, STATUS_FAILED);
            return false;
        }
        true
    }

    pub fn setup_queue(&self, queue_idx: u32, queue_size: u16,
                       desc_phys: u64, avail_phys: u64, used_phys: u64) {
        self.write32(QUEUE_SEL, queue_idx);
        self.write32(QUEUE_NUM, queue_size as u32);
        self.write32(QUEUE_DESC_LOW, desc_phys as u32);
        self.write32(QUEUE_DESC_HIGH, (desc_phys >> 32) as u32);
        self.write32(QUEUE_DRIVER_LOW, avail_phys as u32);
        self.write32(QUEUE_DRIVER_HIGH, (avail_phys >> 32) as u32);
        self.write32(QUEUE_DEVICE_LOW, used_phys as u32);
        self.write32(QUEUE_DEVICE_HIGH, (used_phys >> 32) as u32);
        self.write32(QUEUE_READY, 1);
    }

    pub fn queue_max_size(&self, queue_idx: u32) -> u32 {
        self.write32(QUEUE_SEL, queue_idx);
        self.read32(QUEUE_NUM_MAX)
    }

    pub fn notify(&self, queue_idx: u32) {
        self.write32(QUEUE_NOTIFY, queue_idx);
    }

    pub fn driver_ok(&self) {
        let status = self.read32(STATUS);
        self.write32(STATUS, status | STATUS_DRIVER_OK);
    }

    pub fn ack_interrupt(&self) {
        let status = self.read32(INTERRUPT_STATUS);
        self.write32(INTERRUPT_ACK, status);
    }

    pub fn read_config<T: Copy>(&self, offset: usize) -> T {
        unsafe { ptr::read_volatile((self.base + CONFIG + offset) as *const T) }
    }

    fn read32(&self, offset: usize) -> u32 {
        unsafe { ptr::read_volatile((self.base + offset) as *const u32) }
    }

    fn write32(&self, offset: usize, val: u32) {
        unsafe { ptr::write_volatile((self.base + offset) as *mut u32, val) }
    }
}
