use alloc::collections::VecDeque;
use spin::Mutex;

// struct input_event (64-bit Linux): timeval(16) + type(2) + code(2) + value(4) = 24 bytes
#[repr(C)]
pub struct InputEvent {
    pub time_sec: u64,
    pub time_usec: u64,
    pub ev_type: u16,
    pub code: u16,
    pub value: i32,
}

// Input event type codes
pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const EV_ABS: u16 = 3;

// Relative axis codes
pub const REL_X: u16 = 0;
pub const REL_Y: u16 = 1;

// Absolute axis codes
pub const ABS_X: u16 = 0;
pub const ABS_Y: u16 = 1;

pub const KEY_RELEASE: i32 = 0;
pub const KEY_PRESS: i32 = 1;
pub const KEY_REPEAT: i32 = 2;

// Separate event queues: keyboard (event0, minor 64) and mouse (event1, minor 65)
static KBD_QUEUE: Mutex<VecDeque<InputEvent>> = Mutex::new(VecDeque::new());
static MOUSE_QUEUE: Mutex<VecDeque<InputEvent>> = Mutex::new(VecDeque::new());

// Legacy combined queue for /dev/input/event* catch-all reads (minor >= 66)
static EVENT_QUEUE: Mutex<VecDeque<InputEvent>> = Mutex::new(VecDeque::new());

fn ticks_to_timeval() -> (u64, u64) {
    let ticks = crate::arch::ticks();
    (ticks / 100, (ticks % 100) * 10000)
}

pub fn push_key(keycode: u16, pressed: bool) {
    let (sec, usec) = ticks_to_timeval();
    let mut q = KBD_QUEUE.lock();
    q.push_back(InputEvent {
        time_sec: sec, time_usec: usec,
        ev_type: EV_KEY, code: keycode,
        value: if pressed { KEY_PRESS } else { KEY_RELEASE },
    });
    q.push_back(InputEvent {
        time_sec: sec, time_usec: usec,
        ev_type: EV_SYN, code: 0, value: 0,
    });
    if q.len() > 256 { q.pop_front(); }

    // Also push to legacy combined queue
    let mut eq = EVENT_QUEUE.lock();
    eq.push_back(InputEvent {
        time_sec: sec, time_usec: usec,
        ev_type: EV_KEY, code: keycode,
        value: if pressed { KEY_PRESS } else { KEY_RELEASE },
    });
    eq.push_back(InputEvent {
        time_sec: sec, time_usec: usec,
        ev_type: EV_SYN, code: 0, value: 0,
    });
    if eq.len() > 256 { eq.pop_front(); }
}

pub fn push_mouse_rel(dx: i32, dy: i32, _buttons: u32) {
    let (sec, usec) = ticks_to_timeval();
    let mut q = MOUSE_QUEUE.lock();
    if dx != 0 {
        q.push_back(InputEvent { time_sec: sec, time_usec: usec, ev_type: EV_REL, code: REL_X, value: dx });
    }
    if dy != 0 {
        q.push_back(InputEvent { time_sec: sec, time_usec: usec, ev_type: EV_REL, code: REL_Y, value: dy });
    }
    q.push_back(InputEvent { time_sec: sec, time_usec: usec, ev_type: EV_SYN, code: 0, value: 0 });
    if q.len() > 256 { q.pop_front(); }

    // Also push to legacy combined queue
    let mut eq = EVENT_QUEUE.lock();
    if dx != 0 {
        eq.push_back(InputEvent { time_sec: sec, time_usec: usec, ev_type: EV_REL, code: REL_X, value: dx });
    }
    if dy != 0 {
        eq.push_back(InputEvent { time_sec: sec, time_usec: usec, ev_type: EV_REL, code: REL_Y, value: dy });
    }
    eq.push_back(InputEvent { time_sec: sec, time_usec: usec, ev_type: EV_SYN, code: 0, value: 0 });
    if eq.len() > 256 { eq.pop_front(); }
}

/// Read events from the queue for a specific device minor number.
/// minor 64 = event0 (keyboard), minor 65 = event1 (mouse), others = combined.
pub fn read_events_for_minor(minor: u32, buf: u64, max_bytes: usize) -> usize {
    match minor {
        64 => drain_queue(&KBD_QUEUE, buf, max_bytes),
        65 => drain_queue(&MOUSE_QUEUE, buf, max_bytes),
        _  => drain_queue(&EVENT_QUEUE, buf, max_bytes),
    }
}

/// Legacy: read from combined queue (for callers that don't pass minor)
pub fn read_events(buf: u64, max_bytes: usize) -> usize {
    drain_queue(&EVENT_QUEUE, buf, max_bytes)
}

fn drain_queue(queue: &Mutex<VecDeque<InputEvent>>, buf: u64, max_bytes: usize) -> usize {
    let event_size = core::mem::size_of::<InputEvent>(); // 24
    let max_events = max_bytes / event_size;
    let mut q = queue.lock();
    let count = max_events.min(q.len());
    for i in 0..count {
        let ev = q.pop_front().unwrap();
        let dest = (buf + (i * event_size) as u64) as *mut InputEvent;
        unsafe { core::ptr::write(dest, ev); }
    }
    count * event_size
}

/// Push a raw virtio input event (from virtio-input device).
/// Routes keyboard events to KBD_QUEUE, mouse events to MOUSE_QUEUE.
pub fn push_virtio_event(ev_type: u16, code: u16, value: u32) {
    if ev_type == EV_SYN { return; }
    let (sec, usec) = ticks_to_timeval();
    let ev = InputEvent {
        time_sec: sec, time_usec: usec,
        ev_type, code, value: value as i32,
    };
    let syn = InputEvent {
        time_sec: sec, time_usec: usec,
        ev_type: EV_SYN, code: 0, value: 0,
    };

    match ev_type {
        EV_KEY => {
            let mut q = KBD_QUEUE.lock();
            q.push_back(ev);
            q.push_back(syn);
            if q.len() > 512 { q.pop_front(); }
        }
        EV_REL | EV_ABS => {
            let mut q = MOUSE_QUEUE.lock();
            q.push_back(ev);
            q.push_back(syn);
            if q.len() > 512 { q.pop_front(); }
        }
        _ => {
            // Unknown type -> push to combined
            let mut q = EVENT_QUEUE.lock();
            q.push_back(ev);
            q.push_back(syn);
            if q.len() > 512 { q.pop_front(); }
        }
    }

    // Also push to legacy combined queue
    let (sec2, usec2) = ticks_to_timeval();
    let mut eq = EVENT_QUEUE.lock();
    eq.push_back(InputEvent {
        time_sec: sec2, time_usec: usec2,
        ev_type, code, value: value as i32,
    });
    eq.push_back(InputEvent {
        time_sec: sec2, time_usec: usec2,
        ev_type: EV_SYN, code: 0, value: 0,
    });
    if eq.len() > 512 { eq.pop_front(); }
}

pub fn has_events() -> bool {
    !EVENT_QUEUE.lock().is_empty()
        || !KBD_QUEUE.lock().is_empty()
        || !MOUSE_QUEUE.lock().is_empty()
}

/// Check if a specific minor's queue has events.
pub fn has_events_for_minor(minor: u32) -> bool {
    match minor {
        64 => !KBD_QUEUE.lock().is_empty(),
        65 => !MOUSE_QUEUE.lock().is_empty(),
        _  => !EVENT_QUEUE.lock().is_empty(),
    }
}

/// Handle input device ioctls.
/// `minor` is the device minor number: 64 = event0 (keyboard), 65 = event1 (mouse).
pub fn handle_ioctl_with_minor(minor: u32, request: u64, arg: u64) -> crate::syscall::SyscallResult {
    let is_mouse = minor == 65;

    match request {
        0x80084502 => { // EVIOCGVERSION
            if arg != 0 { unsafe { *(arg as *mut u32) = 0x010001; } }
            Ok(0)
        }
        0x80044501 => { // EVIOCGID
            if arg != 0 {
                unsafe { core::ptr::write_bytes(arg as *mut u8, 0, 8); }
                // struct input_id: bustype(u16) + vendor(u16) + product(u16) + version(u16)
                unsafe {
                    *(arg as *mut u16) = 0x06; // BUS_VIRTUAL
                    *((arg + 2) as *mut u16) = 0x0001; // vendor
                    *((arg + 4) as *mut u16) = if is_mouse { 0x0002 } else { 0x0001 }; // product
                    *((arg + 6) as *mut u16) = 0x0001; // version
                }
            }
            Ok(0)
        }
        // EVIOCGNAME - encoded as _IOC(_IOC_READ, 'E', 0x06, len)
        // The ioctl number encodes the buffer size in bits [29:16].
        // Common pattern: 0x80XX4506 where XX = buffer size
        r if (r & 0xFFFF00FF) == 0x80004506 => {
            if arg != 0 {
                let buf_size = ((r >> 16) & 0xFF) as usize;
                let name: &[u8] = if is_mouse {
                    b"Cyllor Virtual Mouse\0"
                } else {
                    b"Cyllor Virtual Keyboard\0"
                };
                let copy_len = name.len().min(buf_size);
                unsafe {
                    core::ptr::copy_nonoverlapping(name.as_ptr(), arg as *mut u8, copy_len);
                }
                return Ok(copy_len as usize);
            }
            Ok(0)
        }
        // EVIOCGPHYS - 0x80XX4507
        r if (r & 0xFFFF00FF) == 0x80004507 => {
            if arg != 0 {
                let buf_size = ((r >> 16) & 0xFF) as usize;
                let phys: &[u8] = if is_mouse {
                    b"virtual/input1\0"
                } else {
                    b"virtual/input0\0"
                };
                let copy_len = phys.len().min(buf_size);
                unsafe {
                    core::ptr::copy_nonoverlapping(phys.as_ptr(), arg as *mut u8, copy_len);
                }
                return Ok(copy_len as usize);
            }
            Ok(0)
        }
        // EVIOCGBIT(ev, len) - _IOC(_IOC_READ, 'E', 0x20 + ev, len)
        // Encoded as 0x80XX45(20+ev)
        r if (r & 0xFFFF0000) == 0x80004500 || (r & 0xFFFF0000) == 0x80404500
            || (r & 0xFFFF0000) == 0x80604500 || (r & 0xFFFF0000) == 0x80804500 => {
            let ev_type = (r & 0xFF) as u8;
            if ev_type < 0x20 || ev_type > 0x3F {
                // Not EVIOCGBIT range — fall through to catch-all
                return Ok(0);
            }
            let ev = ev_type - 0x20;
            let buf_size = ((r >> 16) & 0xFF) as usize;
            if arg == 0 { return Ok(0); }

            // Zero the output buffer first
            let clear_len = buf_size.min(128);
            unsafe { core::ptr::write_bytes(arg as *mut u8, 0, clear_len); }

            match ev {
                0 => {
                    // EV_SYN (type 0) — report supported event types
                    // Bit 0 = EV_SYN, bit 1 = EV_KEY, bit 2 = EV_REL, bit 3 = EV_ABS
                    if is_mouse {
                        // Mouse: EV_SYN + EV_KEY + EV_REL
                        unsafe { *(arg as *mut u8) = 0x07; } // bits 0,1,2
                    } else {
                        // Keyboard: EV_SYN + EV_KEY
                        unsafe { *(arg as *mut u8) = 0x03; } // bits 0,1
                    }
                }
                1 => {
                    // EV_KEY — report supported key codes
                    // For keyboard: set bits for common keys (0..255)
                    // For mouse: set BTN_LEFT (0x110), BTN_RIGHT (0x111), BTN_MIDDLE (0x112)
                    if is_mouse {
                        // BTN_LEFT=0x110, BTN_RIGHT=0x111, BTN_MIDDLE=0x112
                        // byte 0x110/8 = 0x22 = 34, bit 0x110%8 = 0
                        if buf_size > 34 {
                            unsafe {
                                *((arg as *mut u8).add(34)) = 0x07; // bits 0,1,2 -> BTN_LEFT, RIGHT, MIDDLE
                            }
                        }
                    } else {
                        // Keyboard: set bits for keys 1..127 (standard PC keys)
                        for byte_idx in 0..16usize {
                            if byte_idx < buf_size {
                                unsafe { *((arg as *mut u8).add(byte_idx)) = 0xFF; }
                            }
                        }
                    }
                }
                2 => {
                    // EV_REL — report supported relative axes
                    if is_mouse {
                        // REL_X=0, REL_Y=1
                        unsafe { *(arg as *mut u8) = 0x03; } // bits 0,1
                    }
                }
                3 => {
                    // EV_ABS — report supported absolute axes
                    // (currently not used but available for touchscreen)
                }
                _ => {}
            }
            Ok(0)
        }
        // EVIOCGABS(axis) - _IOC(_IOC_READ, 'E', 0x40 + axis, sizeof(struct input_absinfo))
        // struct input_absinfo: value(i32) + minimum(i32) + maximum(i32) + fuzz(i32) + flat(i32) + resolution(i32) = 24 bytes
        // Encoded as 0x80184540+axis  (size=0x18=24)
        r if (r & 0xFFFFFF00) == 0x80184500 => {
            let axis = (r & 0xFF) as u8;
            if axis < 0x40 {
                return Ok(0); // not EVIOCGABS range
            }
            let abs_code = axis - 0x40;
            if arg == 0 { return Ok(0); }

            // Zero the struct
            unsafe { core::ptr::write_bytes(arg as *mut u8, 0, 24); }

            match abs_code {
                0 => { // ABS_X
                    unsafe {
                        *(arg as *mut i32) = 0;              // value
                        *((arg + 4) as *mut i32) = 0;        // minimum
                        *((arg + 8) as *mut i32) = 32767;    // maximum
                        *((arg + 12) as *mut i32) = 0;       // fuzz
                        *((arg + 16) as *mut i32) = 0;       // flat
                        *((arg + 20) as *mut i32) = 0;       // resolution
                    }
                }
                1 => { // ABS_Y
                    unsafe {
                        *(arg as *mut i32) = 0;
                        *((arg + 4) as *mut i32) = 0;
                        *((arg + 8) as *mut i32) = 32767;
                        *((arg + 12) as *mut i32) = 0;
                        *((arg + 16) as *mut i32) = 0;
                        *((arg + 20) as *mut i32) = 0;
                    }
                }
                _ => {} // unknown axis — leave zeros
            }
            Ok(0)
        }
        // EVIOCGPROP — 0x80004509+size  report device properties (empty)
        r if (r & 0xFFFF00FF) == 0x80000009 || (r & 0xFFFF00FF) == 0x80004509 => {
            if arg != 0 {
                let buf_size = ((r >> 16) & 0xFF) as usize;
                let clear = buf_size.min(64);
                unsafe { core::ptr::write_bytes(arg as *mut u8, 0, clear); }
            }
            Ok(0)
        }
        _ => Err(crate::syscall::ENOTTY),
    }
}

/// Legacy ioctl handler (used when minor is not available).
pub fn handle_ioctl(request: u64, arg: u64) -> crate::syscall::SyscallResult {
    // Default to keyboard (event0, minor 64)
    handle_ioctl_with_minor(64, request, arg)
}
