// DRM/KMS ioctl interface - minimal implementation for Wayland/Weston
use super::framebuffer;
use crate::syscall::{SyscallResult, EINVAL, ENOMEM};
use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use spin::Mutex;

// DRM ioctl numbers (from Linux include/uapi/drm/drm.h)
const DRM_IOCTL_WAIT_VBLANK: u64 = 0xC01C643F;
const DRM_IOCTL_VERSION: u64 = 0xC0406400;
const DRM_IOCTL_GET_CAP: u64 = 0xC010640C;
const DRM_IOCTL_SET_CLIENT_CAP: u64 = 0x4010640D;
const DRM_IOCTL_MODE_GETRESOURCES: u64 = 0xC04064A0;
const DRM_IOCTL_MODE_GETCRTC: u64 = 0xC06864A1;
const DRM_IOCTL_MODE_SETCRTC: u64 = 0xC06864A2;
const DRM_IOCTL_MODE_GETENCODER: u64 = 0xC01464A6;
const DRM_IOCTL_MODE_GETCONNECTOR: u64 = 0xC05064A7;
const DRM_IOCTL_MODE_GETPROPERTY: u64 = 0xC04064AA;
const DRM_IOCTL_MODE_ADDFB: u64 = 0xC01C64AE;
const DRM_IOCTL_MODE_ADDFB2: u64 = 0xC04464B8;
const DRM_IOCTL_MODE_RMFB: u64 = 0xC00464AF;
const DRM_IOCTL_MODE_PAGE_FLIP: u64 = 0xC01064B0;
const DRM_IOCTL_MODE_CREATE_DUMB: u64 = 0xC02064B2;
const DRM_IOCTL_MODE_MAP_DUMB: u64 = 0xC01064B3;
const DRM_IOCTL_MODE_DESTROY_DUMB: u64 = 0xC00464B4;
const DRM_IOCTL_MODE_OBJ_GETPROPERTIES: u64 = 0xC02064B9;
const DRM_IOCTL_MODE_ATOMIC: u64 = 0xC04064BC;
const DRM_IOCTL_PRIME_HANDLE_TO_FD: u64 = 0xC00C642D;
const DRM_IOCTL_PRIME_FD_TO_HANDLE: u64 = 0xC00C642E;
const DRM_IOCTL_GEM_CLOSE: u64 = 0x40086409;

pub struct DumbBuffer {
    pub handle: u32,
    pub phys: u64,
    pub size: usize,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
}

static DUMB_BUFFERS: Mutex<BTreeMap<u32, DumbBuffer>> = Mutex::new(BTreeMap::new());
static NEXT_HANDLE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

/// Tracks fb_id -> dumb buffer handle mapping (created by ADDFB/ADDFB2).
static FB_MAP: Mutex<BTreeMap<u32, u32>> = Mutex::new(BTreeMap::new());
static NEXT_FB_ID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

// DRM event queue for page-flip and vblank events
static DRM_EVENTS: Mutex<VecDeque<[u8; 32]>> = Mutex::new(VecDeque::new());

/// Called from timer tick -- generates a vblank event if the queue is empty
pub fn vblank_tick() {
    static VBLANK_SEQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let mut queue = DRM_EVENTS.lock();
    if queue.is_empty() {
        let seq = VBLANK_SEQ.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let mut ev = [0u8; 32];
        ev[0..4].copy_from_slice(&1u32.to_le_bytes()); // DRM_EVENT_VBLANK
        ev[4..8].copy_from_slice(&32u32.to_le_bytes()); // length
        ev[16..20].copy_from_slice(&seq.to_le_bytes()); // sequence
        queue.push_back(ev);
    }
}

pub fn read_events(buf: u64, count: usize) -> usize {
    let mut queue = DRM_EVENTS.lock();
    let event_size = 32;
    let mut written = 0;
    while written + event_size <= count {
        match queue.pop_front() {
            Some(ev) => {
                unsafe {
                    core::ptr::copy_nonoverlapping(ev.as_ptr(), (buf + written as u64) as *mut u8, event_size);
                }
                written += event_size;
            }
            None => break,
        }
    }
    written
}

pub fn has_events() -> bool {
    !DRM_EVENTS.lock().is_empty()
}

/// Called from do_mmap to map a dumb buffer's physical pages.
pub fn get_dumb_phys(offset: u64) -> Option<(u64, usize)> {
    let bufs = DUMB_BUFFERS.lock();
    for buf in bufs.values() {
        if buf.phys == offset {
            return Some((buf.phys, buf.size));
        }
    }
    None
}

/// Blit a dumb buffer to the display (Limine framebuffer + virtio-gpu).
fn blit_buffer_to_display(db: &DumbBuffer) {
    let hhdm = crate::arch::hhdm_offset();
    let src_ptr = (db.phys + hhdm) as *const u8;
    let src_pitch = db.pitch as usize;

    // Blit to Limine framebuffer
    if let Some(fb_info) = framebuffer::get_info() {
        let dst_ptr = fb_info.addr as *mut u8;
        let dst_pitch = fb_info.pitch as usize;
        let copy_rows = (db.height as usize).min(fb_info.height as usize);
        let copy_bytes = src_pitch.min(dst_pitch);
        for row in 0..copy_rows {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    src_ptr.wrapping_add(row * src_pitch),
                    dst_ptr.wrapping_add(row * dst_pitch),
                    copy_bytes,
                );
            }
        }
    }

    // Also push to virtio-gpu framebuffer
    if let Some((gpu_fb_phys, gpu_w, gpu_h)) = crate::drivers::virtio::gpu::framebuffer_info() {
        let gpu_dst = (gpu_fb_phys + hhdm) as *mut u8;
        let gpu_pitch = gpu_w as usize * 4;
        let blit_w = (db.width as usize).min(gpu_w as usize);
        let blit_h = (db.height as usize).min(gpu_h as usize);
        for row in 0..blit_h {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    src_ptr.wrapping_add(row * src_pitch),
                    gpu_dst.wrapping_add(row * gpu_pitch),
                    blit_w * 4,
                );
            }
        }
    }
}

/// Look up a dumb buffer handle from an fb_id via the FB_MAP.
fn fb_id_to_handle(fb_id: u32) -> Option<u32> {
    FB_MAP.lock().get(&fb_id).copied()
}

pub fn handle_ioctl(request: u64, arg: u64) -> SyscallResult {
    match request {
        DRM_IOCTL_VERSION => {
            if arg != 0 {
                unsafe {
                    let ptr = arg as *mut u8;
                    core::ptr::write_bytes(ptr, 0, 128);
                    *(arg as *mut i32) = 1;
                    *((arg + 4) as *mut i32) = 0;
                    *((arg + 8) as *mut i32) = 0;
                    *((arg + 16) as *mut u64) = 6;
                    *((arg + 32) as *mut u64) = 10;
                }
            }
            Ok(0)
        }
        DRM_IOCTL_GET_CAP => {
            if arg != 0 {
                let cap = unsafe { *(arg as *const u64) };
                let value: u64 = match cap {
                    0x1 => 1, // DRM_CAP_DUMB_BUFFER
                    0x2 => 0, // DRM_CAP_VBLANK_HIGH_CRTC
                    0x3 => 1, // DRM_CAP_DUMB_PREFERRED_DEPTH = 32
                    0x4 => 1, // DRM_CAP_DUMB_PREFER_SHADOW
                    0x6 => 1, // DRM_CAP_PRIME
                    0x10 => 1, // DRM_CAP_TIMESTAMP_MONOTONIC
                    0x12 => 1, // DRM_CAP_ADDFB2_MODIFIERS
                    0x13 => 1, // DRM_CAP_CRTC_IN_VBLANK_EVENT
                    _ => 0,
                };
                unsafe { *((arg + 8) as *mut u64) = value; }
            }
            Ok(0)
        }
        DRM_IOCTL_SET_CLIENT_CAP => Ok(0),
        DRM_IOCTL_MODE_GETRESOURCES => {
            // struct drm_mode_card_res:
            //   fb_id_ptr(u64) + crtc_id_ptr(u64) + connector_id_ptr(u64) + encoder_id_ptr(u64)
            //   + count_fbs(u32) + count_crtcs(u32) + count_connectors(u32) + count_encoders(u32)
            //   + min_width(u32) + max_width(u32) + min_height(u32) + max_height(u32)
            if arg != 0 {
                unsafe {
                    let fb_ptr    = *(arg as *const u64);
                    let crtc_ptr  = *((arg + 8) as *const u64);
                    let conn_ptr  = *((arg + 16) as *const u64);
                    let enc_ptr   = *((arg + 24) as *const u64);

                    let fb_count = FB_MAP.lock().len() as u32;
                    *((arg + 32) as *mut u32) = fb_count; // count_fbs
                    *((arg + 36) as *mut u32) = 1;        // count_crtcs
                    *((arg + 40) as *mut u32) = 1;        // count_connectors
                    *((arg + 44) as *mut u32) = 1;        // count_encoders
                    *((arg + 48) as *mut u32) = 0;        // min_width
                    *((arg + 52) as *mut u32) = 4096;     // max_width
                    *((arg + 56) as *mut u32) = 0;        // min_height
                    *((arg + 60) as *mut u32) = 4096;     // max_height

                    if fb_ptr != 0 {
                        let map = FB_MAP.lock();
                        for (i, fb_id) in map.keys().enumerate() {
                            *((fb_ptr as *mut u32).add(i)) = *fb_id;
                        }
                    }
                    if crtc_ptr != 0 { *(crtc_ptr as *mut u32) = 1; }
                    if conn_ptr != 0 { *(conn_ptr as *mut u32) = 1; }
                    if enc_ptr  != 0 { *(enc_ptr  as *mut u32) = 1; }
                }
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_GETCRTC => {
            if arg != 0 {
                if let Some(fb) = framebuffer::get_info() {
                    unsafe {
                        *((arg + 8) as *mut u32) = 1; // fb_id
                        *((arg + 12) as *mut u32) = 0; // x
                        *((arg + 16) as *mut u32) = 0; // y
                        *((arg + 32) as *mut u32) = 60000; // clock
                        *((arg + 36) as *mut u16) = fb.width as u16;
                        *((arg + 52) as *mut u16) = fb.height as u16;
                        *((arg + 68) as *mut u32) = 60; // vrefresh
                    }
                }
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_GETCONNECTOR => {
            if arg != 0 {
                let modes_ptr = unsafe { *((arg + 8) as *const u64) };
                unsafe {
                    *((arg + 32) as *mut u32) = 1;  // count_modes
                    *((arg + 36) as *mut u32) = 0;  // count_props
                    *((arg + 40) as *mut u32) = 1;  // count_encoders
                    *((arg + 44) as *mut u32) = 1;  // encoder_id
                    *((arg + 48) as *mut u32) = 1;  // connector_id
                    *((arg + 52) as *mut u32) = 11; // connector_type = VIRTUAL
                    *((arg + 56) as *mut u32) = 1;  // connector_type_id
                    *((arg + 60) as *mut u32) = 1;  // connection = connected
                    *((arg + 64) as *mut u32) = 0;  // mm_width
                    *((arg + 68) as *mut u32) = 0;  // mm_height
                }
                if modes_ptr != 0 {
                    if let Some(fb) = framebuffer::get_info() {
                        unsafe {
                            let m = modes_ptr as *mut u8;
                            core::ptr::write_bytes(m, 0, 68);
                            *(modes_ptr as *mut u32) = 60000; // clock
                            *((modes_ptr + 4) as *mut u16) = fb.width as u16;
                            *((modes_ptr + 6) as *mut u16) = fb.width as u16 + 88;
                            *((modes_ptr + 8) as *mut u16) = fb.width as u16 + 132;
                            *((modes_ptr + 10) as *mut u16) = fb.width as u16 + 280;
                            *((modes_ptr + 14) as *mut u16) = fb.height as u16;
                            *((modes_ptr + 16) as *mut u16) = fb.height as u16 + 4;
                            *((modes_ptr + 18) as *mut u16) = fb.height as u16 + 9;
                            *((modes_ptr + 20) as *mut u16) = fb.height as u16 + 28;
                            *((modes_ptr + 24) as *mut u32) = 60; // vrefresh
                            *((modes_ptr + 32) as *mut u32) = 48; // DRM_MODE_TYPE_PREFERRED | DRIVER
                            let name = alloc::format!("{}x{}\0", fb.width, fb.height);
                            let nb = name.as_bytes();
                            core::ptr::copy_nonoverlapping(nb.as_ptr(), (modes_ptr + 36) as *mut u8, nb.len().min(32));
                        }
                    }
                }
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_GETENCODER => {
            if arg != 0 {
                unsafe {
                    *((arg + 4) as *mut u32) = 1; // encoder_type
                    *((arg + 8) as *mut u32) = 1; // crtc_id
                    *((arg + 12) as *mut u32) = 1; // possible_crtcs
                }
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_CREATE_DUMB => {
            if arg != 0 {
                let height = unsafe { *(arg as *const u32) };
                let width  = unsafe { *((arg + 4) as *const u32) };
                let bpp    = unsafe { *((arg + 8) as *const u32) };
                let bpp = if bpp == 0 { 32 } else { bpp };

                let pitch = (width * (bpp / 8) + 63) & !63; // 64-byte aligned
                let size = (height as usize) * (pitch as usize);

                let buf: alloc::boxed::Box<[u8]> = alloc::vec![0u8; size].into_boxed_slice();
                let virt_addr = buf.as_ptr() as u64;
                let hhdm = crate::arch::hhdm_offset();
                let phys_addr = virt_addr - hhdm;
                core::mem::forget(buf);

                let handle = NEXT_HANDLE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

                let db = DumbBuffer { handle, phys: phys_addr, size, width, height, pitch };
                DUMB_BUFFERS.lock().insert(handle, db);

                unsafe {
                    *((arg + 12) as *mut u32) = handle;
                    *((arg + 16) as *mut u32) = pitch;
                    *((arg + 20) as *mut u64) = size as u64;
                }
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_MAP_DUMB => {
            if arg != 0 {
                let handle = unsafe { *(arg as *const u32) };
                let bufs = DUMB_BUFFERS.lock();
                if let Some(db) = bufs.get(&handle) {
                    unsafe { *((arg + 8) as *mut u64) = db.phys; }
                } else if let Some(fb) = framebuffer::get_info() {
                    let hhdm = crate::arch::hhdm_offset();
                    let phys = if fb.addr >= hhdm { fb.addr - hhdm } else { fb.addr };
                    unsafe { *((arg + 8) as *mut u64) = phys; }
                }
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_ADDFB => {
            // struct drm_mode_fb_cmd: width(u32)+height(u32)+pitch(u32)+bpp(u32)+depth(u32)+handle(u32)+fb_id(u32)
            if arg != 0 {
                let handle = unsafe { *((arg + 20) as *const u32) };
                let fb_id = NEXT_FB_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                FB_MAP.lock().insert(fb_id, handle);
                unsafe { *((arg + 24) as *mut u32) = fb_id; }
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_ADDFB2 => {
            // struct drm_mode_fb_cmd2:
            //   fb_id(u32) + width(u32) + height(u32) + pixel_format(u32) + flags(u32)
            //   + handles[4](16 bytes) + pitches[4](16 bytes) + offsets[4](16 bytes) + modifier[4](32 bytes)
            if arg != 0 {
                let handle = unsafe { *((arg + 20) as *const u32) }; // handles[0]
                let fb_id = NEXT_FB_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                FB_MAP.lock().insert(fb_id, handle);
                unsafe { *(arg as *mut u32) = fb_id; }
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_RMFB => {
            if arg != 0 {
                let fb_id = unsafe { *(arg as *const u32) };
                FB_MAP.lock().remove(&fb_id);
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_PAGE_FLIP => {
            // struct drm_mode_crtc_page_flip: crtc_id(u32)+fb_id(u32)+flags(u32)+reserved(u32)+user_data(u64)
            if arg == 0 { return Ok(0); }
            let fb_id = unsafe { *((arg + 4) as *const u32) };
            let _flags = unsafe { *((arg + 8) as *const u32) };
            let user_data = unsafe { *((arg + 16) as *const u64) };

            // Look up and blit the correct buffer via FB_MAP
            if let Some(handle) = fb_id_to_handle(fb_id) {
                let bufs = DUMB_BUFFERS.lock();
                if let Some(db) = bufs.get(&handle) {
                    blit_buffer_to_display(db);
                }
            }

            // Generate flip-complete event
            static FLIP_SEQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
            let seq = FLIP_SEQ.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            let mut ev = [0u8; 32];
            ev[0..4].copy_from_slice(&2u32.to_le_bytes()); // DRM_EVENT_FLIP_COMPLETE
            ev[4..8].copy_from_slice(&32u32.to_le_bytes());
            ev[8..16].copy_from_slice(&user_data.to_le_bytes());
            ev[16..20].copy_from_slice(&seq.to_le_bytes());
            DRM_EVENTS.lock().push_back(ev);
            crate::drivers::virtio::gpu::flush_all();
            Ok(0)
        }
        DRM_IOCTL_MODE_SETCRTC => {
            if arg == 0 { return Ok(0); }
            let fb_id = unsafe { *((arg + 16) as *const u32) };
            let src_x = unsafe { *((arg + 20) as *const u32) } as usize;
            let src_y = unsafe { *((arg + 24) as *const u32) } as usize;

            let fb_info = match framebuffer::get_info() {
                Some(i) => i,
                None => return Ok(0),
            };

            // Look up the dumb buffer handle via FB_MAP, falling back to fb_id as handle
            let handle = fb_id_to_handle(fb_id).unwrap_or(fb_id);
            let bufs = DUMB_BUFFERS.lock();
            let db = bufs.get(&handle).or_else(|| bufs.values().next());
            if let Some(db) = db {
                let hhdm = crate::arch::hhdm_offset();
                let src_ptr = (db.phys + hhdm) as *const u8;
                let dst_ptr = fb_info.addr as *mut u8;
                let src_pitch = db.pitch as usize;
                let dst_pitch = fb_info.pitch as usize;
                let copy_rows = (db.height as usize).min(fb_info.height as usize);
                let copy_bytes = src_pitch.min(dst_pitch);
                for row in 0..copy_rows {
                    let src_row = src_ptr.wrapping_add((src_y + row) * src_pitch + src_x * 4);
                    let dst_row = dst_ptr.wrapping_add(row * dst_pitch);
                    unsafe { core::ptr::copy_nonoverlapping(src_row, dst_row, copy_bytes); }
                }

                if let Some((gpu_fb_phys, gpu_w, gpu_h)) = crate::drivers::virtio::gpu::framebuffer_info() {
                    let gpu_dst = (gpu_fb_phys + hhdm) as *mut u8;
                    let gpu_pitch = gpu_w as usize * 4;
                    let blit_w = (db.width as usize).min(gpu_w as usize);
                    let blit_h = (db.height as usize).min(gpu_h as usize);
                    for row in 0..blit_h {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                src_ptr.wrapping_add(row * src_pitch),
                                gpu_dst.wrapping_add(row * gpu_pitch),
                                blit_w * 4,
                            );
                        }
                    }
                    crate::drivers::virtio::gpu::flush_all();
                }
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_DESTROY_DUMB => {
            if arg != 0 {
                let handle = unsafe { *(arg as *const u32) };
                DUMB_BUFFERS.lock().remove(&handle);
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_OBJ_GETPROPERTIES => Ok(0),
        DRM_IOCTL_MODE_GETPROPERTY => Ok(0),
        // Return -EOPNOTSUPP so Weston falls back to legacy KMS instead of atomic
        DRM_IOCTL_MODE_ATOMIC => Err(crate::syscall::EOPNOTSUPP),
        DRM_IOCTL_PRIME_HANDLE_TO_FD => Ok(0),
        DRM_IOCTL_PRIME_FD_TO_HANDLE => Ok(0),
        DRM_IOCTL_GEM_CLOSE => Ok(0),
        DRM_IOCTL_WAIT_VBLANK => {
            static SEQ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);
            let s = SEQ.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if arg != 0 {
                unsafe {
                    *((arg + 4) as *mut u32) = s;
                    *((arg + 8) as *mut u32) = 0;
                    *((arg + 12) as *mut u32) = 0;
                }
            }
            Ok(0)
        }
        _ => {
            log::trace!("Unknown DRM ioctl: 0x{:x}", request);
            Ok(0)
        }
    }
}
