// DRM/KMS ioctl interface - minimal implementation for Wayland
use super::framebuffer;
use crate::syscall::{SyscallResult, EINVAL};

// DRM ioctl numbers (from Linux include/uapi/drm/drm.h)
const DRM_IOCTL_BASE: u64 = 0x64;

// Common DRM ioctls
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

pub fn handle_ioctl(request: u64, arg: u64) -> SyscallResult {
    match request {
        DRM_IOCTL_VERSION => {
            // Return driver version
            if arg != 0 {
                unsafe {
                    let ptr = arg as *mut u8;
                    core::ptr::write_bytes(ptr, 0, 128);
                    // version_major, version_minor, version_patchlevel
                    *(arg as *mut i32) = 1;
                    *((arg + 4) as *mut i32) = 0;
                    *((arg + 8) as *mut i32) = 0;
                    // name_len
                    *((arg + 16) as *mut u64) = 6;
                    // desc_len
                    *((arg + 32) as *mut u64) = 10;
                }
            }
            Ok(0)
        }
        DRM_IOCTL_GET_CAP => {
            // struct drm_get_cap { u64 capability; u64 value; }
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
            // Return 1 CRTC, 1 encoder, 1 connector, 1 fb
            if arg != 0 {
                unsafe {
                    core::ptr::write_bytes(arg as *mut u8, 0, 64);
                    // count_fbs
                    *((arg + 0) as *mut u32) = 0;
                    // count_crtcs
                    *((arg + 8) as *mut u32) = 1;
                    // count_connectors
                    *((arg + 16) as *mut u32) = 1;
                    // count_encoders
                    *((arg + 24) as *mut u32) = 1;
                    // min/max width/height
                    *((arg + 32) as *mut u32) = 0;
                    *((arg + 36) as *mut u32) = 4096;
                    *((arg + 40) as *mut u32) = 0;
                    *((arg + 44) as *mut u32) = 4096;
                }
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_GETCRTC => {
            if arg != 0 {
                if let Some(fb) = framebuffer::get_info() {
                    unsafe {
                        // struct drm_mode_crtc fields
                        *((arg + 8) as *mut u32) = 1; // fb_id
                        *((arg + 12) as *mut u32) = 0; // x
                        *((arg + 16) as *mut u32) = 0; // y
                        // mode info at offset 32
                        *((arg + 32) as *mut u32) = 60000; // clock
                        *((arg + 36) as *mut u16) = fb.width as u16; // hdisplay
                        *((arg + 52) as *mut u16) = fb.height as u16; // vdisplay
                        *((arg + 68) as *mut u32) = 60; // vrefresh
                    }
                }
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_GETCONNECTOR => {
            if arg != 0 {
                unsafe {
                    // Return connected, with 1 mode
                    *((arg + 0) as *mut u32) = 1; // encoders_ptr (count)
                    *((arg + 24) as *mut u32) = 1; // count_modes
                    *((arg + 36) as *mut u32) = 2; // connection = connected
                    *((arg + 40) as *mut u32) = 1; // encoder_id
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
            // struct drm_mode_create_dumb
            if arg != 0 {
                let height = unsafe { *(arg as *const u32) };
                let width = unsafe { *((arg + 4) as *const u32) };
                let bpp = unsafe { *((arg + 8) as *const u32) };

                let pitch = width * (bpp / 8);
                let size = (height as u64) * (pitch as u64);

                // Allocate memory for the dumb buffer
                let buf = alloc::vec![0u8; size as usize];
                let ptr = buf.as_ptr() as u64;
                core::mem::forget(buf);

                unsafe {
                    *((arg + 12) as *mut u32) = 1; // handle
                    *((arg + 16) as *mut u32) = pitch; // pitch
                    *((arg + 20) as *mut u64) = size; // size
                }
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_MAP_DUMB => {
            // Return the framebuffer address for mmap
            if arg != 0 {
                if let Some(fb) = framebuffer::get_info() {
                    unsafe {
                        *((arg + 8) as *mut u64) = fb.addr; // offset for mmap
                    }
                }
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_ADDFB | DRM_IOCTL_MODE_ADDFB2 => {
            // Add framebuffer - return fb_id = 1
            if arg != 0 {
                unsafe {
                    // For ADDFB: fb_id is at offset 24
                    *((arg + 24) as *mut u32) = 1;
                }
            }
            Ok(0)
        }
        DRM_IOCTL_MODE_RMFB => Ok(0),
        DRM_IOCTL_MODE_PAGE_FLIP => Ok(0),
        DRM_IOCTL_MODE_SETCRTC => Ok(0),
        DRM_IOCTL_MODE_DESTROY_DUMB => Ok(0),
        DRM_IOCTL_MODE_OBJ_GETPROPERTIES => Ok(0),
        DRM_IOCTL_MODE_GETPROPERTY => Ok(0),
        DRM_IOCTL_MODE_ATOMIC => Ok(0),
        DRM_IOCTL_PRIME_HANDLE_TO_FD => Ok(0),
        DRM_IOCTL_PRIME_FD_TO_HANDLE => Ok(0),
        DRM_IOCTL_GEM_CLOSE => Ok(0),
        _ => {
            // Accept unknown DRM ioctls silently
            log::trace!("Unknown DRM ioctl: 0x{:x}", request);
            Ok(0)
        }
    }
}
