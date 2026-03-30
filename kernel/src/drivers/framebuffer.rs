// Framebuffer driver using Limine's framebuffer protocol
use spin::Mutex;

pub static FB: Mutex<Option<FramebufferInfo>> = Mutex::new(None);

#[derive(Debug, Clone)]
pub struct FramebufferInfo {
    pub addr: u64,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub size: usize,
}

#[used]
#[unsafe(link_section = ".requests")]
static FB_REQUEST: limine::request::FramebufferRequest = limine::request::FramebufferRequest::new();

pub fn init() {
    let response = match FB_REQUEST.response() {
        Some(r) => r,
        None => {
            log::warn!("No framebuffer available");
            return;
        }
    };

    let fbs = response.framebuffers();
    if fbs.is_empty() {
        log::warn!("No framebuffers returned");
        return;
    }

    let fb = &fbs[0];
    let info = FramebufferInfo {
        addr: fb.address() as u64,
        width: fb.width,
        height: fb.height,
        pitch: fb.pitch,
        bpp: fb.bpp,
        size: fb.size(),
    };

    log::info!(
        "Framebuffer: {}x{} @ {bpp}bpp, pitch={}, addr=0x{:x}",
        info.width, info.height, info.pitch, info.addr,
        bpp = info.bpp
    );

    // Clear to dark blue
    let fb_ptr = info.addr as *mut u32;
    let pixels = (info.pitch / 4 * info.height) as usize;
    for i in 0..pixels {
        unsafe { *fb_ptr.add(i) = 0x002244; }
    }

    *FB.lock() = Some(info);
}

/// Get framebuffer info for DRM/KMS ioctl
pub fn get_info() -> Option<FramebufferInfo> {
    FB.lock().clone()
}

/// Write pixel data to framebuffer
pub fn write_pixels(offset: usize, data: &[u8]) -> usize {
    let fb = FB.lock();
    if let Some(ref info) = *fb {
        let max = info.size;
        let len = data.len().min(max.saturating_sub(offset));
        if len > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    (info.addr as usize + offset) as *mut u8,
                    len,
                );
            }
        }
        len
    } else {
        0
    }
}

/// mmap the framebuffer into userspace
pub fn mmap_framebuffer() -> Option<(usize, usize)> {
    let fb = FB.lock();
    if let Some(ref info) = *fb {
        Some((info.addr as usize, info.size))
    } else {
        None
    }
}
