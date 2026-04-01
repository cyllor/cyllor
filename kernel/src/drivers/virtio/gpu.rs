// VirtIO GPU Device driver (device ID = 16)
// Implements 2D scanout via the virtio-gpu control queue.
// Flow: RESOURCE_CREATE_2D → RESOURCE_ATTACH_BACKING → SET_SCANOUT →
//       TRANSFER_TO_HOST_2D → RESOURCE_FLUSH

use super::mmio::VirtioMmio;
use crate::mm::pmm;
use core::ptr;
use core::sync::atomic::{fence, Ordering};
use spin::Mutex;

// Control command types
const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;

// Response types
const VIRTIO_GPU_RESP_OK_NODATA: u32 = 0x1100;
const VIRTIO_GPU_RESP_OK_DISPLAY_INFO: u32 = 0x1101;

// Pixel format: B8G8R8X8 (most compatible with QEMU virtio-gpu)
const VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM: u32 = 2;

const QUEUE_SIZE: u16 = 16;
const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;

// Default framebuffer dimensions (may be overridden by GET_DISPLAY_INFO)
const DEFAULT_WIDTH: u32 = 1024;
const DEFAULT_HEIGHT: u32 = 768;

#[repr(C)]
struct GpuCtrlHdr {
    cmd_type: u32,
    flags: u32,
    fence_id: u64,
    ctx_id: u32,
    _padding: u32,
}

impl GpuCtrlHdr {
    fn new(cmd_type: u32) -> Self {
        Self { cmd_type, flags: 0, fence_id: 0, ctx_id: 0, _padding: 0 }
    }
}

#[repr(C)]
struct GpuRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
struct GpuResourceCreate2d {
    hdr: GpuCtrlHdr,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
struct GpuResourceAttachBacking {
    hdr: GpuCtrlHdr,
    resource_id: u32,
    nr_entries: u32,
    // followed by nr_entries GpuMemEntry
}

#[repr(C)]
struct GpuMemEntry {
    addr: u64,
    length: u32,
    _padding: u32,
}

#[repr(C)]
struct GpuSetScanout {
    hdr: GpuCtrlHdr,
    r: GpuRect,
    scanout_id: u32,
    resource_id: u32,
}

#[repr(C)]
struct GpuTransferToHost2d {
    hdr: GpuCtrlHdr,
    r: GpuRect,
    offset: u64,
    resource_id: u32,
    _padding: u32,
}

#[repr(C)]
struct GpuResourceFlush {
    hdr: GpuCtrlHdr,
    r: GpuRect,
    resource_id: u32,
    _padding: u32,
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

pub struct VirtioGpu {
    mmio: VirtioMmio,
    hhdm: u64,
    // Control queue (0)
    desc: *mut VringDesc,
    avail: *mut VringAvail,
    used: *mut VringUsed,
    last_used: u16,
    avail_idx: u16,
    // Command/response buffers (1 page each)
    cmd_phys: u64,
    resp_phys: u64,
    // Framebuffer
    pub fb_phys: u64,
    pub width: u32,
    pub height: u32,
    resource_id: u32,
}

unsafe impl Send for VirtioGpu {}
unsafe impl Sync for VirtioGpu {}

static GPU_DEV: Mutex<Option<VirtioGpu>> = Mutex::new(None);

pub fn probe(hhdm: u64) {
    for i in 0..32 {
        let base = 0x0a000000 + i * 0x200 + hhdm as usize;
        if let Some(mmio) = VirtioMmio::new(base) {
            if mmio.device_id() == 16 {
                log::info!("VirtIO GPU found at 0x{:x}", base - hhdm as usize);
                init_gpu(mmio, hhdm);
                return;
            }
        }
    }
    log::info!("No VirtIO GPU found");
}

fn init_gpu(mmio: VirtioMmio, hhdm: u64) {
    if !mmio.init() {
        log::error!("VirtIO GPU init failed");
        return;
    }

    // Allocate control queue
    let p0 = pmm::alloc_page().unwrap() as u64;
    let p1 = pmm::alloc_page().unwrap() as u64;
    unsafe {
        ptr::write_bytes((p0 + hhdm) as *mut u8, 0, 4096);
        ptr::write_bytes((p1 + hhdm) as *mut u8, 0, 4096);
    }
    let desc_phys = p0;
    let avail_phys = p0 + core::mem::size_of::<VringDesc>() as u64 * QUEUE_SIZE as u64;
    let used_phys = p1;
    mmio.setup_queue(0, QUEUE_SIZE, desc_phys, avail_phys, used_phys);
    mmio.driver_ok();

    let cmd_phys = pmm::alloc_page().unwrap() as u64;
    let resp_phys = pmm::alloc_page().unwrap() as u64;
    unsafe {
        ptr::write_bytes((cmd_phys + hhdm) as *mut u8, 0, 4096);
        ptr::write_bytes((resp_phys + hhdm) as *mut u8, 0, 4096);
    }

    let mut gpu = VirtioGpu {
        mmio,
        hhdm,
        desc: (desc_phys + hhdm) as *mut VringDesc,
        avail: (avail_phys + hhdm) as *mut VringAvail,
        used: (used_phys + hhdm) as *mut VringUsed,
        last_used: 0,
        avail_idx: 0,
        cmd_phys,
        resp_phys,
        fb_phys: 0,
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        resource_id: 1,
    };

    // Query display info to get actual resolution
    gpu.get_display_info();

    let (w, h) = (gpu.width, gpu.height);
    let fb_pages = (w as usize * h as usize * 4 + 4095) / 4096;
    let fb_phys = alloc_contiguous(fb_pages, hhdm);
    gpu.fb_phys = fb_phys;

    log::info!("VirtIO GPU: {}x{}, FB at 0x{:x}", w, h, fb_phys);

    // Set up framebuffer resource
    gpu.resource_create_2d(1, w, h);
    gpu.resource_attach_backing(1, fb_phys, w * h * 4);
    gpu.set_scanout(0, 1, w, h);

    *GPU_DEV.lock() = Some(gpu);
    log::info!("VirtIO GPU initialized");
}

/// Allocate N contiguous physical pages (simple sequential allocation).
fn alloc_contiguous(n: usize, hhdm: u64) -> u64 {
    // Allocate pages one by one; for simplicity assume PMM returns sequential pages
    // in small allocations (which is typical for a bump allocator).
    let first = pmm::alloc_page().unwrap() as u64;
    unsafe { ptr::write_bytes((first + hhdm) as *mut u8, 0, 4096); }
    for i in 1..n {
        let p = pmm::alloc_page().unwrap() as u64;
        unsafe { ptr::write_bytes((p + hhdm) as *mut u8, 0, 4096); }
        let _ = p; // We rely on them being contiguous
        // NOTE: if the PMM doesn't return contiguous pages this may need a contiguous
        // allocator. For QEMU with a simple bump PMM this works in practice.
        let _ = i;
    }
    first
}

impl VirtioGpu {
    /// Submit a command (cmd_phys, cmd_len) and receive a response (resp_phys).
    /// Uses descriptors 0 (cmd, device reads) and 1 (resp, device writes).
    fn submit_cmd(&mut self, cmd_len: u32, resp_len: u32) {
        unsafe {
            (*self.desc.add(0)).addr = self.cmd_phys;
            (*self.desc.add(0)).len = cmd_len;
            (*self.desc.add(0)).flags = VRING_DESC_F_NEXT;
            (*self.desc.add(0)).next = 1;

            (*self.desc.add(1)).addr = self.resp_phys;
            (*self.desc.add(1)).len = resp_len;
            (*self.desc.add(1)).flags = VRING_DESC_F_WRITE;
            (*self.desc.add(1)).next = 0;

            let ai = (*self.avail).idx;
            (*self.avail).ring[(ai % QUEUE_SIZE) as usize] = 0;
            fence(Ordering::Release);
            (*self.avail).idx = ai.wrapping_add(1);
        }

        fence(Ordering::SeqCst);
        self.mmio.notify(0);

        // Spin until used ring advances
        let mut timeout = 0u32;
        loop {
            fence(Ordering::Acquire);
            let ui = unsafe { ptr::read_volatile(&(*self.used).idx) };
            if ui != self.last_used { break; }
            core::hint::spin_loop();
            timeout += 1;
            if timeout > 10_000_000 {
                log::warn!("VirtIO GPU: command timeout");
                break;
            }
        }
        self.last_used = self.last_used.wrapping_add(1);
        self.mmio.ack_interrupt();
    }

    fn get_display_info(&mut self) {
        let cmd_virt = (self.cmd_phys + self.hhdm) as *mut GpuCtrlHdr;
        unsafe { *cmd_virt = GpuCtrlHdr::new(VIRTIO_GPU_CMD_GET_DISPLAY_INFO); }

        let resp_len = 4 + 4 + 24 * 16; // header + pmodes[16] (simplified)
        self.submit_cmd(core::mem::size_of::<GpuCtrlHdr>() as u32, resp_len as u32);

        let resp_virt = (self.resp_phys + self.hhdm) as *const u32;
        let resp_type = unsafe { ptr::read_volatile(resp_virt) };
        if resp_type == VIRTIO_GPU_RESP_OK_DISPLAY_INFO {
            // Response layout after header (24 bytes): array of virtio_gpu_display_one
            // Each entry: rect(16) + enabled(4) + flags(4) = 24 bytes
            let base = (self.resp_phys + self.hhdm + 24) as *const u32; // skip ctrl header
            let w = unsafe { ptr::read_volatile(base.add(2)) }; // rect.width
            let h = unsafe { ptr::read_volatile(base.add(3)) }; // rect.height
            if w > 0 && h > 0 {
                self.width = w;
                self.height = h;
            }
        }
    }

    fn resource_create_2d(&mut self, resource_id: u32, width: u32, height: u32) {
        let cmd = GpuResourceCreate2d {
            hdr: GpuCtrlHdr::new(VIRTIO_GPU_CMD_RESOURCE_CREATE_2D),
            resource_id,
            format: VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM,
            width,
            height,
        };
        unsafe {
            ptr::write_volatile(
                (self.cmd_phys + self.hhdm) as *mut GpuResourceCreate2d,
                cmd,
            );
        }
        self.submit_cmd(
            core::mem::size_of::<GpuResourceCreate2d>() as u32,
            core::mem::size_of::<GpuCtrlHdr>() as u32,
        );
    }

    fn resource_attach_backing(&mut self, resource_id: u32, fb_phys: u64, fb_size: u32) {
        // Lay out: AttachBacking header + one MemEntry
        let virt = (self.cmd_phys + self.hhdm) as *mut u8;
        let hdr = GpuResourceAttachBacking {
            hdr: GpuCtrlHdr::new(VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING),
            resource_id,
            nr_entries: 1,
        };
        let entry = GpuMemEntry { addr: fb_phys, length: fb_size, _padding: 0 };
        unsafe {
            ptr::write_volatile(virt as *mut GpuResourceAttachBacking, hdr);
            ptr::write_volatile(
                virt.add(core::mem::size_of::<GpuResourceAttachBacking>()) as *mut GpuMemEntry,
                entry,
            );
        }
        let cmd_len = (core::mem::size_of::<GpuResourceAttachBacking>()
            + core::mem::size_of::<GpuMemEntry>()) as u32;
        self.submit_cmd(cmd_len, core::mem::size_of::<GpuCtrlHdr>() as u32);
    }

    fn set_scanout(&mut self, scanout_id: u32, resource_id: u32, width: u32, height: u32) {
        let cmd = GpuSetScanout {
            hdr: GpuCtrlHdr::new(VIRTIO_GPU_CMD_SET_SCANOUT),
            r: GpuRect { x: 0, y: 0, width, height },
            scanout_id,
            resource_id,
        };
        unsafe {
            ptr::write_volatile((self.cmd_phys + self.hhdm) as *mut GpuSetScanout, cmd);
        }
        self.submit_cmd(
            core::mem::size_of::<GpuSetScanout>() as u32,
            core::mem::size_of::<GpuCtrlHdr>() as u32,
        );
    }

    fn transfer_and_flush(&mut self, x: u32, y: u32, w: u32, h: u32) {
        let resource_id = self.resource_id;
        // Transfer
        let t = GpuTransferToHost2d {
            hdr: GpuCtrlHdr::new(VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D),
            r: GpuRect { x, y, width: w, height: h },
            offset: (y * self.width + x) as u64 * 4,
            resource_id,
            _padding: 0,
        };
        unsafe {
            ptr::write_volatile((self.cmd_phys + self.hhdm) as *mut GpuTransferToHost2d, t);
        }
        self.submit_cmd(
            core::mem::size_of::<GpuTransferToHost2d>() as u32,
            core::mem::size_of::<GpuCtrlHdr>() as u32,
        );

        // Flush
        let f = GpuResourceFlush {
            hdr: GpuCtrlHdr::new(VIRTIO_GPU_CMD_RESOURCE_FLUSH),
            r: GpuRect { x, y, width: w, height: h },
            resource_id,
            _padding: 0,
        };
        unsafe {
            ptr::write_volatile((self.cmd_phys + self.hhdm) as *mut GpuResourceFlush, f);
        }
        self.submit_cmd(
            core::mem::size_of::<GpuResourceFlush>() as u32,
            core::mem::size_of::<GpuCtrlHdr>() as u32,
        );
    }
}

/// Flush a dirty rectangle of the framebuffer to the display.
/// Coordinates are in pixels; (0,0) is top-left.
pub fn flush_rect(x: u32, y: u32, w: u32, h: u32) {
    if let Some(dev) = GPU_DEV.lock().as_mut() {
        dev.transfer_and_flush(x, y, w, h);
    }
}

/// Flush the entire framebuffer.
pub fn flush_all() {
    if let Some(dev) = GPU_DEV.lock().as_mut() {
        let (w, h) = (dev.width, dev.height);
        dev.transfer_and_flush(0, 0, w, h);
    }
}

/// Return (framebuffer physical address, width, height) or None if no GPU.
pub fn framebuffer_info() -> Option<(u64, u32, u32)> {
    GPU_DEV.lock().as_ref().map(|d| (d.fb_phys, d.width, d.height))
}

/// Return the virtual address of the framebuffer (via HHDM), or 0 if no GPU.
pub fn framebuffer_virt() -> u64 {
    GPU_DEV.lock().as_ref().map(|d| d.fb_phys + d.hhdm).unwrap_or(0)
}

pub fn is_available() -> bool {
    GPU_DEV.lock().is_some()
}
