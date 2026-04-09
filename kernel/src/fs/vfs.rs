use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::syscall::{SyscallResult, ENOENT, EEXIST, ENOTDIR, EISDIR, EINVAL, ENOSYS};

/// Inode types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InodeType {
    File,
    Directory,
    CharDevice,
    BlockDevice,
    Pipe,
    Socket,
    Symlink,
}

/// An inode in the VFS
pub struct Inode {
    pub itype: InodeType,
    pub size: usize,
    pub data: Vec<u8>,
    pub children: BTreeMap<String, Arc<Mutex<Inode>>>,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub dev_major: u32,
    pub dev_minor: u32,
}

impl Inode {
    pub fn new_dir(mode: u32) -> Self {
        Self {
            itype: InodeType::Directory,
            size: 0,
            data: Vec::new(),
            children: BTreeMap::new(),
            mode,
            uid: 0, gid: 0,
            dev_major: 0, dev_minor: 0,
        }
    }

    pub fn new_file(mode: u32) -> Self {
        Self {
            itype: InodeType::File,
            size: 0,
            data: Vec::new(),
            children: BTreeMap::new(),
            mode,
            uid: 0, gid: 0,
            dev_major: 0, dev_minor: 0,
        }
    }

    pub fn new_chardev(major: u32, minor: u32) -> Self {
        Self {
            itype: InodeType::CharDevice,
            size: 0,
            data: Vec::new(),
            children: BTreeMap::new(),
            mode: 0o666,
            uid: 0, gid: 0,
            dev_major: major, dev_minor: minor,
        }
    }
}

/// File object - represents an open file
pub struct FileObject {
    pub inode: Option<Arc<Mutex<Inode>>>,
    pub offset: usize,
    pub flags: u32,
    pub ftype: FileType,
    // For special files
    pub special_data: Option<SpecialData>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileType {
    Regular,
    Directory,
    CharDevice,
    Pipe,
    Epoll,
    EventFd,
    TimerFd,
    SignalFd,
    MemFd,
    PidFd,
    Socket,
    PtyMaster,
    PtySlave,
}

pub enum SpecialData {
    EventFdVal(u64),
    TimerFdState {
        next_deadline: u64,
        interval_ticks: u64,
        pending_expirations: u64,
    },
    SignalFdMask(u64),
    PipeBuffer(Arc<Mutex<Vec<u8>>>),
    MemFdData(Vec<u8>),
    PidFd(u64),
    PtyMaster(u32),
    PtySlave(u32),
}

impl FileObject {
    pub fn new(inode: Arc<Mutex<Inode>>, flags: u32) -> Self {
        let itype = inode.lock().itype;
        let ftype = match itype {
            InodeType::File => FileType::Regular,
            InodeType::Directory => FileType::Directory,
            InodeType::CharDevice => FileType::CharDevice,
            _ => FileType::Regular,
        };
        Self { inode: Some(inode), offset: 0, flags, ftype, special_data: None }
    }

    pub fn new_epoll() -> Self {
        Self { inode: None, offset: 0, flags: 0, ftype: FileType::Epoll, special_data: None }
    }

    pub fn new_eventfd(initval: u32) -> Self {
        Self {
            inode: None, offset: 0, flags: 0, ftype: FileType::EventFd,
            special_data: Some(SpecialData::EventFdVal(initval as u64)),
        }
    }

    pub fn new_timerfd() -> Self {
        Self {
            inode: None,
            offset: 0,
            flags: 0,
            ftype: FileType::TimerFd,
            special_data: Some(SpecialData::TimerFdState {
                next_deadline: 0,
                interval_ticks: 0,
                pending_expirations: 0,
            }),
        }
    }

    pub fn new_signalfd(mask: u64) -> Self {
        Self {
            inode: None,
            offset: 0,
            flags: 0,
            ftype: FileType::SignalFd,
            special_data: Some(SpecialData::SignalFdMask(mask)),
        }
    }

    pub fn new_memfd() -> Self {
        Self {
            inode: None, offset: 0, flags: 0, ftype: FileType::MemFd,
            special_data: Some(SpecialData::MemFdData(Vec::new())),
        }
    }

    pub fn new_pidfd(pid: u64) -> Self {
        Self {
            inode: None,
            offset: 0,
            flags: 0,
            ftype: FileType::PidFd,
            special_data: Some(SpecialData::PidFd(pid)),
        }
    }

    pub fn new_pipe(buf: Arc<Mutex<Vec<u8>>>, is_read: bool) -> Self {
        Self {
            inode: None, offset: 0,
            flags: if is_read { 0 } else { 1 },
            ftype: FileType::Pipe,
            special_data: Some(SpecialData::PipeBuffer(buf)),
        }
    }

    pub fn read(&mut self, buf: u64, count: usize) -> SyscallResult {
        match self.ftype {
            FileType::Regular | FileType::MemFd => {
                let data = if let Some(ref inode) = self.inode {
                    let node = inode.lock();
                    let avail = node.data.len().saturating_sub(self.offset);
                    let to_read = count.min(avail);
                    if to_read > 0 {
                        let src = &node.data[self.offset..self.offset + to_read];
                        let _ = crate::syscall::fs::copy_to_user(buf, src);
                    }
                    self.offset += to_read;
                    to_read
                } else if let Some(SpecialData::MemFdData(ref data)) = self.special_data {
                    let avail = data.len().saturating_sub(self.offset);
                    let to_read = count.min(avail);
                    if to_read > 0 {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                data[self.offset..].as_ptr(), buf as *mut u8, to_read,
                            );
                        }
                    }
                    self.offset += to_read;
                    to_read
                } else {
                    0
                };
                Ok(data)
            }
            FileType::CharDevice => {
                if let Some(ref inode) = self.inode {
                    let (dev_major, dev_minor) = {
                        let node = inode.lock();
                        (node.dev_major, node.dev_minor)
                    };
                    match (dev_major, dev_minor) {
                        (1, 3) => Ok(0), // /dev/null
                        (1, 5) => { // /dev/zero
                            if count > 0 {
                                unsafe { core::ptr::write_bytes(buf as *mut u8, 0, count); }
                            }
                            Ok(count)
                        }
                        (1, 9) => { // /dev/urandom
                            let mut seed = crate::arch::ticks();
                            let out = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, count) };
                            for b in out.iter_mut() {
                                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                                *b = (seed >> 33) as u8;
                            }
                            Ok(count)
                        }
                        (13, minor @ 64..=127) => {
                            // /dev/input/event* — read from per-device queue
                            let n = crate::drivers::input::read_events_for_minor(minor, buf, count);
                            Ok(n)
                        }
                        (226, _) => {
                            // DRM device: read pending events
                            let n = crate::drivers::drm::read_events(buf, count);
                            Ok(n)
                        }
                        (5, _) => {
                            // /dev/tty behaves like the console slave PTY.
                            let pty_id = crate::drivers::pty::console_pty_id();
                            loop {
                                if let Some(pty_arc) = crate::drivers::pty::get_pty(pty_id) {
                                    let pty = pty_arc.lock();
                                    let mut rb = pty.slave_rx.lock();
                                    if !rb.is_empty() {
                                        let to_read = count.min(rb.len());
                                        let _ = crate::syscall::fs::copy_to_user(buf, &rb[..to_read]);
                                        rb.drain(..to_read);
                                        break Ok(to_read);
                                    }
                                }
                                if self.flags & 0o4000 != 0 {
                                    break Err(crate::syscall::EAGAIN);
                                }
                                crate::sched::wait::sleep_ticks(1);
                            }
                        }
                        _ => Ok(0),
                    }
                } else {
                    Ok(0)
                }
            }
            FileType::Pipe => {
                if let Some(SpecialData::PipeBuffer(ref pipe_buf)) = self.special_data {
                    let mut pb = pipe_buf.lock();
                    let to_read = count.min(pb.len());
                    if to_read > 0 {
                        unsafe {
                            core::ptr::copy_nonoverlapping(pb.as_ptr(), buf as *mut u8, to_read);
                        }
                        pb.drain(..to_read);
                    }
                    Ok(to_read)
                } else {
                    Ok(0)
                }
            }
            FileType::EventFd => {
                if count < 8 { return Err(EINVAL); }
                if let Some(SpecialData::EventFdVal(ref mut val)) = self.special_data {
                    unsafe { *(buf as *mut u64) = *val; }
                    *val = 0;
                    Ok(8)
                } else {
                    Ok(0)
                }
            }
            FileType::TimerFd => {
                if count < 8 {
                    return Err(EINVAL);
                }
                loop {
                    let now = crate::arch::read_counter();
                    if let Some(SpecialData::TimerFdState {
                        ref mut next_deadline,
                        interval_ticks,
                        ref mut pending_expirations,
                    }) = self.special_data
                    {
                        if *next_deadline != 0 && now >= *next_deadline {
                            if interval_ticks == 0 {
                                *pending_expirations = pending_expirations.saturating_add(1);
                                *next_deadline = 0;
                            } else {
                                let delta = now.saturating_sub(*next_deadline);
                                let periods = delta / interval_ticks + 1;
                                *pending_expirations = pending_expirations.saturating_add(periods);
                                *next_deadline = next_deadline.saturating_add(periods.saturating_mul(interval_ticks));
                            }
                        }
                        if *pending_expirations > 0 {
                            let out = *pending_expirations;
                            *pending_expirations = 0;
                            let bytes = out.to_ne_bytes();
                            let _ = crate::syscall::fs::copy_to_user(buf, &bytes);
                            return Ok(8);
                        }
                    }
                    if self.flags & 0o4000 != 0 {
                        return Err(crate::syscall::EAGAIN);
                    }
                    crate::sched::wait::sleep_ticks(1);
                }
            }
            FileType::SignalFd => {
                if count < 128 {
                    return Err(EINVAL);
                }
                let mask = match self.special_data {
                    Some(SpecialData::SignalFdMask(m)) => m,
                    _ => 0,
                };
                loop {
                    if let Some(sig) = crate::ipc::signal::signalfd_consume(mask) {
                        let mut info = [0u8; 128];
                        info[0..4].copy_from_slice(&(sig as u32).to_le_bytes()); // ssi_signo
                        crate::syscall::fs::copy_to_user(buf, &info).map_err(|_| EINVAL)?;
                        return Ok(128);
                    }
                    if self.flags & 0o4000 != 0 {
                        return Err(crate::syscall::EAGAIN);
                    }
                    crate::sched::wait::sleep_ticks(1);
                }
            }
            FileType::PtyMaster => {
                let pty_id = match &self.special_data {
                    Some(SpecialData::PtyMaster(id)) => *id,
                    _ => return Ok(0),
                };
                loop {
                    if let Some(pty_arc) = crate::drivers::pty::get_pty(pty_id) {
                        let pty = pty_arc.lock();
                        let mut rb = pty.master_rx.lock();
                        if !rb.is_empty() {
                            let to_read = count.min(rb.len());
                            let _ = crate::syscall::fs::copy_to_user(buf, &rb[..to_read]);
                            rb.drain(..to_read);
                            return Ok(to_read);
                        }
                    }
                    // Non-blocking check
                    if self.flags & 0o4000 != 0 { return Err(crate::syscall::EAGAIN); }
                    crate::sched::wait::sleep_ticks(1);
                }
            }
            FileType::PtySlave => {
                let pty_id = match &self.special_data {
                    Some(SpecialData::PtySlave(id)) => *id,
                    _ => return Ok(0),
                };
                loop {
                    if let Some(pty_arc) = crate::drivers::pty::get_pty(pty_id) {
                        let pty = pty_arc.lock();
                        let mut rb = pty.slave_rx.lock();
                        if !rb.is_empty() {
                            let to_read = count.min(rb.len());
                            let _ = crate::syscall::fs::copy_to_user(buf, &rb[..to_read]);
                            rb.drain(..to_read);
                            return Ok(to_read);
                        }
                    }
                    if self.flags & 0o4000 != 0 { return Err(crate::syscall::EAGAIN); }
                    crate::sched::wait::sleep_ticks(1);
                }
            }
            _ => Ok(0),
        }
    }

    pub fn write(&mut self, buf: u64, count: usize) -> SyscallResult {
        match self.ftype {
            FileType::Regular | FileType::MemFd => {
                if let Some(ref inode) = self.inode {
                    let mut node = inode.lock();
                    let end = self.offset + count;
                    if end > node.data.len() {
                        node.data.resize(end, 0);
                    }
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            buf as *const u8, node.data[self.offset..].as_mut_ptr(), count,
                        );
                    }
                    node.size = node.data.len();
                    self.offset = end;
                } else if let Some(SpecialData::MemFdData(ref mut data)) = self.special_data {
                    let end = self.offset + count;
                    if end > data.len() {
                        data.resize(end, 0);
                    }
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            buf as *const u8, data[self.offset..].as_mut_ptr(), count,
                        );
                    }
                    self.offset = end;
                }
                Ok(count)
            }
            FileType::CharDevice => {
                // stdout/stderr handled in syscall layer
                Ok(count)
            }
            FileType::Pipe => {
                if let Some(SpecialData::PipeBuffer(ref pipe_buf)) = self.special_data {
                    let mut pb = pipe_buf.lock();
                    let slice = unsafe { core::slice::from_raw_parts(buf as *const u8, count) };
                    pb.extend_from_slice(slice);
                    Ok(count)
                } else {
                    Ok(0)
                }
            }
            FileType::EventFd => {
                if count < 8 { return Err(EINVAL); }
                let val = unsafe { *(buf as *const u64) };
                if let Some(SpecialData::EventFdVal(ref mut v)) = self.special_data {
                    *v = v.wrapping_add(val);
                }
                Ok(8)
            }
            FileType::TimerFd | FileType::SignalFd => Err(crate::syscall::EINVAL),
            FileType::PtyMaster => {
                // Master write → line discipline → slave_rx
                if let Some(SpecialData::PtyMaster(id)) = &self.special_data {
                    let data = unsafe { core::slice::from_raw_parts(buf as *const u8, count) };
                    crate::drivers::pty::master_write(*id, data);
                }
                Ok(count)
            }
            FileType::PtySlave => {
                // Slave write → output processing (c_oflag) → master_rx
                if let Some(SpecialData::PtySlave(id)) = &self.special_data {
                    let data = unsafe { core::slice::from_raw_parts(buf as *const u8, count) };
                    crate::drivers::pty::slave_write(*id, data);
                    // For the console PTY, also echo to UART so output is visible
                    if *id == crate::drivers::pty::console_pty_id() {
                        if let Some(pty_arc) = crate::drivers::pty::get_pty(*id) {
                            let pty = pty_arc.lock();
                            let processed = crate::drivers::pty::process_output(&pty, data);
                            for &b in &processed {
                                crate::drivers::uart::write_byte(b);
                            }
                        }
                    }
                }
                Ok(count)
            }
            _ => Ok(count),
        }
    }

    pub fn lseek(&mut self, offset: i64, whence: u32) -> SyscallResult {
        let size = if let Some(ref inode) = self.inode {
            inode.lock().data.len()
        } else {
            0
        };

        let new_offset = match whence {
            0 => offset as usize,                          // SEEK_SET
            1 => (self.offset as i64 + offset) as usize,   // SEEK_CUR
            2 => (size as i64 + offset) as usize,           // SEEK_END
            _ => return Err(EINVAL),
        };

        self.offset = new_offset;
        Ok(new_offset)
    }

    pub fn stat(&self, statbuf: u64) -> SyscallResult {
        // Build stat struct in kernel memory, then copy to user
        let mut buf = [0u8; 128];

        let mode: u32 = if let Some(ref inode) = self.inode {
            let node = inode.lock();
            let type_bits = match node.itype {
                InodeType::File => 0o100000,
                InodeType::Directory => 0o040000,
                InodeType::CharDevice => 0o020000,
                _ => 0o100000,
            };
            type_bits | node.mode
        } else {
            match self.ftype {
                FileType::Pipe => 0o010000 | 0o600,
                _ => 0o100000 | 0o644,
            }
        };
        buf[16..20].copy_from_slice(&mode.to_le_bytes());

        let size: u64 = if let Some(ref inode) = self.inode {
            inode.lock().size as u64
        } else { 0 };
        buf[48..56].copy_from_slice(&size.to_le_bytes());

        // st_blksize
        buf[56..64].copy_from_slice(&4096u64.to_le_bytes());

        crate::syscall::fs::copy_to_user(statbuf, &buf).map_err(|_| EINVAL)?;
        Ok(0)
    }

    pub fn ioctl(&mut self, request: u64, arg: u64) -> SyscallResult {
        // ---- PTY-specific ioctls (master or slave) ----
        if self.ftype == FileType::PtyMaster || self.ftype == FileType::PtySlave {
            let id = match &self.special_data {
                Some(SpecialData::PtyMaster(id)) | Some(SpecialData::PtySlave(id)) => *id,
                _ => return Ok(0),
            };

            match request {
                // --- termios ---
                0x5401 => { // TCGETS
                    if arg != 0 {
                        if let Some(pty_arc) = crate::drivers::pty::get_pty(id) {
                            let pty = pty_arc.lock();
                            let buf = pty.termios.to_bytes();
                            unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), arg as *mut u8, 60); }
                        }
                    }
                    return Ok(0);
                }
                0x5402 | 0x5403 | 0x5404 => { // TCSETS / TCSETSW / TCSETSF
                    if arg != 0 {
                        if let Some(pty_arc) = crate::drivers::pty::get_pty(id) {
                            let mut buf = [0u8; 60];
                            unsafe { core::ptr::copy_nonoverlapping(arg as *const u8, buf.as_mut_ptr(), 60); }
                            let mut pty = pty_arc.lock();
                            pty.termios = crate::drivers::pty::Termios::from_bytes(&buf);
                        }
                    }
                    return Ok(0);
                }
                0x5407 | 0x540B => return Ok(0), // TCSETAF, TCFLSH

                // --- window size ---
                0x5413 => { // TIOCGWINSZ
                    if arg != 0 {
                        if let Some(pty_arc) = crate::drivers::pty::get_pty(id) {
                            let pty = pty_arc.lock();
                            unsafe {
                                *((arg + 0) as *mut u16) = pty.winsize.ws_row;
                                *((arg + 2) as *mut u16) = pty.winsize.ws_col;
                                *((arg + 4) as *mut u16) = pty.winsize.ws_xpixel;
                                *((arg + 6) as *mut u16) = pty.winsize.ws_ypixel;
                            }
                        }
                    }
                    return Ok(0);
                }
                0x5414 => { // TIOCSWINSZ
                    if arg != 0 {
                        if let Some(pty_arc) = crate::drivers::pty::get_pty(id) {
                            let mut pty = pty_arc.lock();
                            unsafe {
                                pty.winsize.ws_row = *((arg + 0) as *const u16);
                                pty.winsize.ws_col = *((arg + 2) as *const u16);
                                pty.winsize.ws_xpixel = *((arg + 4) as *const u16);
                                pty.winsize.ws_ypixel = *((arg + 6) as *const u16);
                            }
                        }
                    }
                    return Ok(0);
                }

                // --- controlling terminal ---
                0x540E => { // TIOCSCTTY
                    if let Some(pty_arc) = crate::drivers::pty::get_pty(id) {
                        let mut pty = pty_arc.lock();
                        let caller = crate::sched::process::current_pid();
                        let (sid, pgid) = {
                            let table = crate::sched::process::PROCESS_TABLE.lock();
                            if let Some(proc) = table.get(&caller) {
                                (proc.sid as i32, proc.pgid as i32)
                            } else {
                                (caller as i32, caller as i32)
                            }
                        };
                        pty.session = sid;
                        pty.fg_pgrp = pgid;
                    }
                    return Ok(0);
                }

                // --- foreground process group ---
                0x540F => { // TIOCGPGRP
                    if arg != 0 {
                        if let Some(pty_arc) = crate::drivers::pty::get_pty(id) {
                            let pty = pty_arc.lock();
                            unsafe { *(arg as *mut i32) = pty.fg_pgrp; }
                        }
                    }
                    return Ok(0);
                }
                0x5410 => { // TIOCSPGRP
                    if arg != 0 {
                        if let Some(pty_arc) = crate::drivers::pty::get_pty(id) {
                            let mut pty = pty_arc.lock();
                            unsafe { pty.fg_pgrp = *(arg as *const i32); }
                        }
                    }
                    return Ok(0);
                }

                // --- PTY number / lock ---
                0x80045430 => { // TIOCGPTN - get PTY number
                    if arg != 0 { unsafe { *(arg as *mut u32) = id; } }
                    return Ok(0);
                }
                0x40045431 => { // TIOCSPTLCK - lock/unlock PTY
                    if arg != 0 {
                        if let Some(pty_arc) = crate::drivers::pty::get_pty(id) {
                            let mut pty = pty_arc.lock();
                            let val = unsafe { *(arg as *const i32) };
                            pty.locked = val != 0;
                        }
                    }
                    return Ok(0);
                }

                0x5441 => { // TIOCGPTPEER - return a slave fd
                    let file = Arc::new(Mutex::new(FileObject {
                        inode: None, offset: 0, flags: 0,
                        ftype: FileType::PtySlave,
                        special_data: Some(SpecialData::PtySlave(id)),
                    }));
                    return crate::fs::fdtable::alloc_fd(file);
                }

                // --- FIONREAD ---
                0x541B => {
                    if arg != 0 {
                        if let Some(pty_arc) = crate::drivers::pty::get_pty(id) {
                            let pty = pty_arc.lock();
                            let count = if self.ftype == FileType::PtyMaster {
                                pty.master_rx.lock().len()
                            } else {
                                pty.slave_rx.lock().len()
                            };
                            unsafe { *(arg as *mut i32) = count as i32; }
                        }
                    }
                    return Ok(0);
                }

                // TIOCGSID — get session ID
                0x5429 => {
                    if arg != 0 {
                        if let Some(pty_arc) = crate::drivers::pty::get_pty(id) {
                            let pty = pty_arc.lock();
                            unsafe { *(arg as *mut i32) = pty.session; }
                        }
                    }
                    return Ok(0);
                }

                _ => {
                    // Fall through to common ioctls below
                }
            }
        }

        // ---- DRM device ioctls ----
        if let Some(ref inode) = self.inode {
            let node = inode.lock();
            if node.dev_major == 226 {
                return crate::drivers::drm::handle_ioctl(request, arg);
            }
            // ---- Input device ioctls ----
            if node.dev_major == 13 && node.dev_minor >= 64 {
                return crate::drivers::input::handle_ioctl_with_minor(node.dev_minor, request, arg);
            }
        }

        // ---- Common terminal ioctls (for non-PTY ttys, e.g. /dev/tty) ----
        let console_id = crate::drivers::pty::console_pty_id();
        match request {
            0x5401 => { // TCGETS
                if arg != 0 {
                    if let Some(pty_arc) = crate::drivers::pty::get_pty(console_id) {
                        let pty = pty_arc.lock();
                        let buf = pty.termios.to_bytes();
                        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), arg as *mut u8, 60); }
                    } else {
                        let buf = crate::drivers::pty::Termios::default_cooked().to_bytes();
                        unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), arg as *mut u8, 60); }
                    }
                }
                Ok(0)
            }
            0x5402 | 0x5403 | 0x5404 => { // TCSETS / TCSETSW / TCSETSF
                if arg != 0 {
                    if let Some(pty_arc) = crate::drivers::pty::get_pty(console_id) {
                        let mut buf = [0u8; 60];
                        unsafe { core::ptr::copy_nonoverlapping(arg as *const u8, buf.as_mut_ptr(), 60); }
                        let mut pty = pty_arc.lock();
                        pty.termios = crate::drivers::pty::Termios::from_bytes(&buf);
                    }
                }
                Ok(0)
            }
            0x5407 | 0x540B => Ok(0), // TCSETAF, TCFLSH
            0x540E => {
                if let Some(pty_arc) = crate::drivers::pty::get_pty(console_id) {
                    let mut pty = pty_arc.lock();
                    let caller = crate::sched::process::current_pid();
                    let (sid, pgid) = {
                        let table = crate::sched::process::PROCESS_TABLE.lock();
                        if let Some(proc) = table.get(&caller) {
                            (proc.sid as i32, proc.pgid as i32)
                        } else {
                            (caller as i32, caller as i32)
                        }
                    };
                    pty.session = sid;
                    pty.fg_pgrp = pgid;
                }
                Ok(0)
            }
            0x540F => { // TIOCGPGRP
                if arg != 0 {
                    if let Some(pty_arc) = crate::drivers::pty::get_pty(console_id) {
                        let pty = pty_arc.lock();
                        unsafe { *(arg as *mut i32) = pty.fg_pgrp; }
                    } else {
                        unsafe { *(arg as *mut i32) = 1; }
                    }
                }
                Ok(0)
            }
            0x5410 => { // TIOCSPGRP
                if arg != 0 {
                    if let Some(pty_arc) = crate::drivers::pty::get_pty(console_id) {
                        let mut pty = pty_arc.lock();
                        unsafe { pty.fg_pgrp = *(arg as *const i32); }
                    }
                }
                Ok(0)
            }
            0x5413 => { // TIOCGWINSZ
                if arg != 0 {
                    if let Some(pty_arc) = crate::drivers::pty::get_pty(console_id) {
                        let pty = pty_arc.lock();
                        unsafe {
                            *((arg + 0) as *mut u16) = pty.winsize.ws_row;
                            *((arg + 2) as *mut u16) = pty.winsize.ws_col;
                            *((arg + 4) as *mut u16) = pty.winsize.ws_xpixel;
                            *((arg + 6) as *mut u16) = pty.winsize.ws_ypixel;
                        }
                    } else {
                        unsafe {
                            *((arg + 0) as *mut u16) = 24;
                            *((arg + 2) as *mut u16) = 80;
                            *((arg + 4) as *mut u16) = 640;
                            *((arg + 6) as *mut u16) = 384;
                        }
                    }
                }
                Ok(0)
            }
            0x5414 => { // TIOCSWINSZ
                if arg != 0 {
                    if let Some(pty_arc) = crate::drivers::pty::get_pty(console_id) {
                        let mut pty = pty_arc.lock();
                        unsafe {
                            pty.winsize.ws_row = *((arg + 0) as *const u16);
                            pty.winsize.ws_col = *((arg + 2) as *const u16);
                        }
                    }
                }
                Ok(0)
            }
            0x5405 | 0x5406 => { // legacy TIOCSPGRP / TIOCGPGRP
                if arg != 0 { unsafe { *(arg as *mut i32) = 1; } }
                Ok(0)
            }
            0x541B => { // FIONREAD
                if arg != 0 { unsafe { *(arg as *mut i32) = 0; } }
                Ok(0)
            }
            _ => Err(crate::syscall::ENOTTY),
        }
    }
}

// Global root filesystem
static ROOT: Mutex<Option<Arc<Mutex<Inode>>>> = Mutex::new(None);
static CWD: Mutex<String> = Mutex::new(String::new());

pub fn init() {
    let mut root = Inode::new_dir(0o755);
    // Create standard directories
    for dir in &["dev", "proc", "tmp", "etc", "var", "home", "usr", "bin", "sbin", "lib", "sys", "run"] {
        root.children.insert(dir.to_string(), Arc::new(Mutex::new(Inode::new_dir(0o755))));
    }

    // Create /usr/bin, /usr/lib, etc.
    if let Some(usr) = root.children.get("usr") {
        let mut usr_node = usr.lock();
        usr_node.children.insert("bin".to_string(), Arc::new(Mutex::new(Inode::new_dir(0o755))));
        usr_node.children.insert("lib".to_string(), Arc::new(Mutex::new(Inode::new_dir(0o755))));
        usr_node.children.insert("share".to_string(), Arc::new(Mutex::new(Inode::new_dir(0o755))));
    }

    *ROOT.lock() = Some(Arc::new(Mutex::new(root)));
    *CWD.lock() = "/".to_string();
}

pub fn root() -> Arc<Mutex<Inode>> {
    ROOT.lock().as_ref().unwrap().clone()
}

pub fn current_dir() -> String {
    CWD.lock().clone()
}

pub fn set_current_dir(path: &str) -> SyscallResult {
    // Verify path exists and is a directory
    let node = resolve_path(path)?;
    let n = node.lock();
    if n.itype != InodeType::Directory {
        return Err(ENOTDIR);
    }
    drop(n);
    *CWD.lock() = path.to_string();
    Ok(0)
}

pub fn resolve_path(path: &str) -> Result<Arc<Mutex<Inode>>, i32> {
    let root = root();

    if path == "/" || path.is_empty() {
        return Ok(root);
    }

    let path = path.trim_start_matches('/');
    let mut current = root;

    for component in path.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            continue;
        }

        let next = {
            let node = current.lock();
            if node.itype != InodeType::Directory {
                return Err(ENOTDIR);
            }
            node.children.get(component).cloned()
        };

        match next {
            Some(child) => {
                // Follow symlinks
                let is_symlink = child.lock().itype == InodeType::Symlink;
                if is_symlink {
                    let target = {
                        let n = child.lock();
                        alloc::string::String::from_utf8_lossy(&n.data).into_owned()
                    };
                    current = resolve_path(&target)?;
                } else {
                    current = child;
                }
            }
            None => return Err(ENOENT),
        }
    }

    Ok(current)
}

/// Resolve a path but do NOT follow the final component if it is a symlink.
/// Used by readlinkat to read the symlink target.
pub fn resolve_path_lstat(path: &str) -> Result<Arc<Mutex<Inode>>, i32> {
    if path == "/" || path.is_empty() {
        return Ok(root());
    }

    let trimmed = path.trim_start_matches('/');
    if let Some(pos) = trimmed.rfind('/') {
        let parent_path = &trimmed[..pos];
        let last = &trimmed[pos + 1..];
        // Resolve the parent (follows symlinks)
        let parent_node = resolve_path(&alloc::format!("/{}", parent_path))?;
        let pn = parent_node.lock();
        match pn.children.get(last) {
            Some(child) => Ok(child.clone()),
            None => Err(ENOENT),
        }
    } else {
        // Single component, parent is root
        let r = root();
        let rn = r.lock();
        match rn.children.get(trimmed) {
            Some(child) => Ok(child.clone()),
            None => Err(ENOENT),
        }
    }
}

pub fn open_node(inode: Arc<Mutex<Inode>>, flags: u32) -> Result<Arc<Mutex<FileObject>>, i32> {
    Ok(Arc::new(Mutex::new(FileObject::new(inode, flags))))
}

pub fn stat_node(inode: &Arc<Mutex<Inode>>, statbuf: u64) -> SyscallResult {
    let node = inode.lock();
    let mut buf = [0u8; 128];
    let type_bits: u32 = match node.itype {
        InodeType::File => 0o100000,
        InodeType::Directory => 0o040000,
        InodeType::CharDevice => 0o020000,
        _ => 0o100000,
    };
    buf[16..20].copy_from_slice(&(type_bits | node.mode).to_le_bytes());
    buf[48..56].copy_from_slice(&(node.size as u64).to_le_bytes());
    buf[56..64].copy_from_slice(&4096u64.to_le_bytes());
    drop(node);
    crate::syscall::fs::copy_to_user(statbuf, &buf).map_err(|_| EINVAL)?;
    Ok(0)
}

pub fn mkdir(path: &str, mode: u32) -> SyscallResult {
    let (parent_path, name) = rsplit_path(path);
    let parent = resolve_path(parent_path)?;
    let mut parent_node = parent.lock();

    if parent_node.children.contains_key(name) {
        return Err(EEXIST);
    }

    parent_node.children.insert(name.to_string(), Arc::new(Mutex::new(Inode::new_dir(mode))));
    Ok(0)
}

pub fn unlink(path: &str, flags: u32) -> SyscallResult {
    let (parent_path, name) = rsplit_path(path);
    let parent = resolve_path(parent_path)?;
    let mut parent_node = parent.lock();

    if parent_node.children.remove(name).is_some() {
        Ok(0)
    } else {
        Err(ENOENT)
    }
}

/// Best-effort absolute path reconstruction for an inode by walking from root.
/// Returns None when the inode is not reachable from the current VFS tree.
pub fn path_of_inode(target: &Arc<Mutex<Inode>>) -> Option<String> {
    let root_node = root();
    if Arc::ptr_eq(&root_node, target) {
        return Some("/".to_string());
    }

    let mut stack: Vec<(Arc<Mutex<Inode>>, String)> = Vec::new();
    stack.push((root_node, "/".to_string()));

    while let Some((node, path)) = stack.pop() {
        if Arc::ptr_eq(&node, target) {
            return Some(path);
        }

        let children: Vec<(String, Arc<Mutex<Inode>>)> = {
            let n = node.lock();
            if n.itype != InodeType::Directory {
                Vec::new()
            } else {
                n.children
                    .iter()
                    .map(|(name, child)| (name.clone(), child.clone()))
                    .collect()
            }
        };

        for (name, child) in children {
            let child_path = if path == "/" {
                alloc::format!("/{}", name)
            } else {
                alloc::format!("{}/{}", path, name)
            };
            stack.push((child, child_path));
        }
    }

    None
}

pub fn rename(old_path: &str, new_path: &str, flags: u32) -> SyscallResult {
    const RENAME_NOREPLACE: u32 = 1;
    if (flags & !RENAME_NOREPLACE) != 0 {
        return Err(EINVAL);
    }
    let (old_parent_path, old_name) = rsplit_path(old_path);
    let (new_parent_path, new_name) = rsplit_path(new_path);
    if old_name.is_empty() || new_name.is_empty() {
        return Err(EINVAL);
    }

    let old_parent = resolve_path(old_parent_path)?;
    let new_parent = resolve_path(new_parent_path)?;

    if Arc::ptr_eq(&old_parent, &new_parent) {
        let mut p = old_parent.lock();
        if (flags & RENAME_NOREPLACE) != 0 && p.children.contains_key(new_name) {
            return Err(EEXIST);
        }
        let moved = p.children.remove(old_name).ok_or(ENOENT)?;
        p.children.insert(new_name.to_string(), moved);
        return Ok(0);
    }

    let moved = {
        let mut op = old_parent.lock();
        op.children.remove(old_name).ok_or(ENOENT)?
    };
    let mut np = new_parent.lock();
    if (flags & RENAME_NOREPLACE) != 0 && np.children.contains_key(new_name) {
        return Err(EEXIST);
    }
    np.children.insert(new_name.to_string(), moved);
    Ok(0)
}

fn rsplit_path(path: &str) -> (&str, &str) {
    let path = path.trim_end_matches('/');
    match path.rfind('/') {
        Some(pos) => {
            let parent = if pos == 0 { "/" } else { &path[..pos] };
            (&parent, &path[pos + 1..])
        }
        None => ("/", path),
    }
}
