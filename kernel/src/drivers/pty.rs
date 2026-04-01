use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

// ---------------------------------------------------------------------------
// termios flags — Linux-compatible values for AArch64
// ---------------------------------------------------------------------------

// c_iflag bits
pub const IGNBRK:  u32 = 0o000001;
pub const BRKINT:  u32 = 0o000002;
pub const IGNPAR:  u32 = 0o000004;
pub const PARMRK:  u32 = 0o000010;
pub const INPCK:   u32 = 0o000020;
pub const ISTRIP:  u32 = 0o000040;
pub const INLCR:   u32 = 0o000100;
pub const IGNCR:   u32 = 0o000200;
pub const ICRNL:   u32 = 0o000400;
pub const IXON:    u32 = 0o002000;
pub const IXOFF:   u32 = 0o010000;
pub const IMAXBEL: u32 = 0o020000;
pub const IUTF8:   u32 = 0o040000;

// c_oflag bits
pub const OPOST:  u32 = 0o000001;
pub const ONLCR:  u32 = 0o000004;

// c_cflag bits
pub const CSIZE:  u32 = 0o000060;
pub const CS8:    u32 = 0o000060;
pub const CREAD:  u32 = 0o000200;
pub const HUPCL:  u32 = 0o002000;
pub const CLOCAL: u32 = 0o004000;
pub const B38400: u32 = 0o000017;

// c_lflag bits
pub const ISIG:    u32 = 0o000001;
pub const ICANON:  u32 = 0o000002;
pub const ECHO:    u32 = 0o000010;
pub const ECHOE:   u32 = 0o000020;
pub const ECHOK:   u32 = 0o000040;
pub const ECHONL:  u32 = 0o000100;
pub const ECHOCTL: u32 = 0o001000;
pub const ECHOKE:  u32 = 0o004000;
pub const IEXTEN:  u32 = 0o100000;

// c_cc indices (Linux AArch64 layout)
pub const VINTR:    usize = 0;
pub const VQUIT:    usize = 1;
pub const VERASE:   usize = 2;
pub const VKILL:    usize = 3;
pub const VEOF:     usize = 4;
pub const VTIME:    usize = 5;
pub const VMIN:     usize = 6;
pub const VSWTC:    usize = 7;
pub const VSTART:   usize = 8;
pub const VSTOP:    usize = 9;
pub const VSUSP:    usize = 10;
pub const VEOL:     usize = 11;
pub const VREPRINT: usize = 12;
pub const VDISCARD: usize = 13;
pub const VWERASE:  usize = 14;
pub const VLNEXT:   usize = 15;
pub const VEOL2:    usize = 16;
pub const NCCS:     usize = 19;

// ---------------------------------------------------------------------------
// termios struct — matches Linux struct termios on AArch64
// Layout: c_iflag(4) + c_oflag(4) + c_cflag(4) + c_lflag(4) + c_line(1) +
//         c_cc[19] + pad(1) + c_ispeed(4) + c_cflag(4) = 60 bytes
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line:  u8,
    pub c_cc:    [u8; NCCS],
    pub _pad:    [u8; 1],  // alignment padding (kernel_termios has this)
    pub c_ispeed: u32,
    pub c_ospeed: u32,
}

impl Termios {
    /// Default termios — sane cooked-mode terminal like Linux
    pub fn default_cooked() -> Self {
        let mut cc = [0u8; NCCS];
        cc[VINTR]    = 0x03; // Ctrl-C
        cc[VQUIT]    = 0x1C; // Ctrl-backslash
        cc[VERASE]   = 0x7F; // DEL
        cc[VKILL]    = 0x15; // Ctrl-U
        cc[VEOF]     = 0x04; // Ctrl-D
        cc[VTIME]    = 0;
        cc[VMIN]     = 1;
        cc[VSWTC]    = 0;
        cc[VSTART]   = 0x11; // Ctrl-Q
        cc[VSTOP]    = 0x13; // Ctrl-S
        cc[VSUSP]    = 0x1A; // Ctrl-Z
        cc[VEOL]     = 0;
        cc[VREPRINT] = 0x12; // Ctrl-R
        cc[VDISCARD] = 0x0F; // Ctrl-O
        cc[VWERASE]  = 0x17; // Ctrl-W
        cc[VLNEXT]   = 0x16; // Ctrl-V
        cc[VEOL2]    = 0;

        Self {
            c_iflag: ICRNL | IXON | IMAXBEL | IUTF8,
            c_oflag: OPOST | ONLCR,
            c_cflag: B38400 | CS8 | CREAD | HUPCL | CLOCAL,
            c_lflag: ISIG | ICANON | ECHO | ECHOE | ECHOK | ECHOCTL | ECHOKE | IEXTEN,
            c_line: 0,
            c_cc: cc,
            _pad: [0],
            c_ispeed: 38400,
            c_ospeed: 38400,
        }
    }

    /// Serialize to 60-byte buffer (Linux kernel_termios layout)
    pub fn to_bytes(&self) -> [u8; 60] {
        let mut buf = [0u8; 60];
        buf[0..4].copy_from_slice(&self.c_iflag.to_le_bytes());
        buf[4..8].copy_from_slice(&self.c_oflag.to_le_bytes());
        buf[8..12].copy_from_slice(&self.c_cflag.to_le_bytes());
        buf[12..16].copy_from_slice(&self.c_lflag.to_le_bytes());
        buf[16] = self.c_line;
        buf[17..17 + NCCS].copy_from_slice(&self.c_cc);
        // buf[36] = pad
        buf[40..44].copy_from_slice(&self.c_ispeed.to_le_bytes());
        buf[44..48].copy_from_slice(&self.c_ospeed.to_le_bytes());
        buf
    }

    /// Deserialize from 60-byte buffer
    pub fn from_bytes(buf: &[u8; 60]) -> Self {
        let mut cc = [0u8; NCCS];
        cc.copy_from_slice(&buf[17..17 + NCCS]);
        Self {
            c_iflag: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            c_oflag: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            c_cflag: u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
            c_lflag: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
            c_line: buf[16],
            c_cc: cc,
            _pad: [0],
            c_ispeed: u32::from_le_bytes([buf[40], buf[41], buf[42], buf[43]]),
            c_ospeed: u32::from_le_bytes([buf[44], buf[45], buf[46], buf[47]]),
        }
    }
}

// ---------------------------------------------------------------------------
// Window size (struct winsize)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Winsize {
    pub ws_row:    u16,
    pub ws_col:    u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

impl Winsize {
    pub fn default_80x24() -> Self {
        Self { ws_row: 24, ws_col: 80, ws_xpixel: 640, ws_ypixel: 384 }
    }
}

// ---------------------------------------------------------------------------
// PTY pair
// ---------------------------------------------------------------------------

pub struct Pty {
    pub id: u32,
    /// master reads this (slave writes here, after line discipline)
    pub master_rx: Arc<Mutex<Vec<u8>>>,
    /// slave reads this (master writes here, after line discipline)
    pub slave_rx: Arc<Mutex<Vec<u8>>>,
    /// Terminal attributes
    pub termios: Termios,
    /// Window size
    pub winsize: Winsize,
    /// Foreground process group ID
    pub fg_pgrp: i32,
    /// Session ID (controlling terminal)
    pub session: i32,
    /// PTY slave is locked (until TIOCSPTLCK with arg=0)
    pub locked: bool,
    /// Line buffer for canonical mode (accumulated until newline)
    pub line_buf: Vec<u8>,
}

static PTYS: Mutex<BTreeMap<u32, Arc<Mutex<Pty>>>> = Mutex::new(BTreeMap::new());
static NEXT_PTY: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The PTY ID that is connected to the system console (UART).
/// Set once by `create_console_pty()`.
static CONSOLE_PTY: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(u32::MAX);

pub fn alloc_pty() -> u32 {
    let id = NEXT_PTY.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let pty = Pty {
        id,
        master_rx: Arc::new(Mutex::new(Vec::new())),
        slave_rx: Arc::new(Mutex::new(Vec::new())),
        termios: Termios::default_cooked(),
        winsize: Winsize::default_80x24(),
        fg_pgrp: 1, // init process group
        session: 1,
        locked: true,
        line_buf: Vec::new(),
    };
    PTYS.lock().insert(id, Arc::new(Mutex::new(pty)));
    id
}

pub fn get_pty(id: u32) -> Option<Arc<Mutex<Pty>>> {
    PTYS.lock().get(&id).cloned()
}

/// Create the console PTY (pty 0) and return its ID.
/// The UART RX interrupt handler will push bytes into this PTY.
pub fn create_console_pty() -> u32 {
    let id = alloc_pty();
    // Unlock it immediately (console is always open)
    if let Some(pty_arc) = get_pty(id) {
        pty_arc.lock().locked = false;
    }
    CONSOLE_PTY.store(id, core::sync::atomic::Ordering::Relaxed);
    log::info!("Console PTY created: /dev/pts/{}", id);
    id
}

/// Get the console PTY ID (u32::MAX if none)
pub fn console_pty_id() -> u32 {
    CONSOLE_PTY.load(core::sync::atomic::Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// UART → PTY bridge: called from UART RX interrupt
// ---------------------------------------------------------------------------

/// Push a raw byte from the UART into the console PTY's slave input path.
/// This goes through the line discipline so bash sees cooked input.
pub fn push_uart_byte(byte: u8) {
    let id = console_pty_id();
    if id == u32::MAX { return; }
    if let Some(pty_arc) = get_pty(id) {
        let mut pty = pty_arc.lock();
        input_byte_ldisc(&mut pty, byte);
    }
}

// ---------------------------------------------------------------------------
// Line discipline — processes input from master→slave direction
// ---------------------------------------------------------------------------

/// Process one input byte through the line discipline.
/// This handles ICRNL, ECHO, ICANON, ISIG (Ctrl-C, Ctrl-Z, etc.)
fn input_byte_ldisc(pty: &mut Pty, mut byte: u8) {
    let iflag = pty.termios.c_iflag;
    let lflag = pty.termios.c_lflag;
    let cc = pty.termios.c_cc;

    // --- Input processing (c_iflag) ---
    if iflag & ICRNL != 0 && byte == b'\r' {
        byte = b'\n';
    }
    if iflag & IGNCR != 0 && byte == b'\r' {
        return;
    }
    if iflag & INLCR != 0 && byte == b'\n' {
        byte = b'\r';
    }

    // --- Signal generation (c_lflag ISIG) ---
    if lflag & ISIG != 0 {
        if byte == cc[VINTR] {
            // Send SIGINT to foreground process group
            echo_byte(pty, b'^');
            echo_byte(pty, b'C');
            echo_byte(pty, b'\n');
            send_signal_to_fg(pty.fg_pgrp, 2); // SIGINT
            // Flush line buffer
            pty.line_buf.clear();
            return;
        }
        if byte == cc[VQUIT] {
            echo_byte(pty, b'^');
            echo_byte(pty, b'\\');
            echo_byte(pty, b'\n');
            send_signal_to_fg(pty.fg_pgrp, 3); // SIGQUIT
            pty.line_buf.clear();
            return;
        }
        if byte == cc[VSUSP] {
            echo_byte(pty, b'^');
            echo_byte(pty, b'Z');
            echo_byte(pty, b'\n');
            send_signal_to_fg(pty.fg_pgrp, 20); // SIGTSTP
            pty.line_buf.clear();
            return;
        }
    }

    // --- Canonical mode (ICANON) ---
    if lflag & ICANON != 0 {
        // Handle VERASE (backspace/delete)
        if byte == cc[VERASE] {
            if !pty.line_buf.is_empty() {
                pty.line_buf.pop();
                if lflag & ECHO != 0 && lflag & ECHOE != 0 {
                    // Erase character on terminal: BS SP BS
                    echo_byte(pty, 0x08);
                    echo_byte(pty, b' ');
                    echo_byte(pty, 0x08);
                }
            }
            return;
        }

        // Handle VKILL (kill line)
        if byte == cc[VKILL] {
            let count = pty.line_buf.len();
            pty.line_buf.clear();
            if lflag & ECHO != 0 && lflag & ECHOK != 0 {
                for _ in 0..count {
                    echo_byte(pty, 0x08);
                    echo_byte(pty, b' ');
                    echo_byte(pty, 0x08);
                }
            }
            return;
        }

        // Handle VWERASE (word erase)
        if byte == cc[VWERASE] {
            // Erase trailing spaces then word
            while !pty.line_buf.is_empty() && *pty.line_buf.last().unwrap() == b' ' {
                pty.line_buf.pop();
                if lflag & ECHO != 0 {
                    echo_byte(pty, 0x08);
                    echo_byte(pty, b' ');
                    echo_byte(pty, 0x08);
                }
            }
            while !pty.line_buf.is_empty() && *pty.line_buf.last().unwrap() != b' ' {
                pty.line_buf.pop();
                if lflag & ECHO != 0 {
                    echo_byte(pty, 0x08);
                    echo_byte(pty, b' ');
                    echo_byte(pty, 0x08);
                }
            }
            return;
        }

        // Handle VEOF (Ctrl-D): flush buffer without adding the byte; if empty → EOF
        if byte == cc[VEOF] {
            let data: Vec<u8> = pty.line_buf.drain(..).collect();
            // Push whatever is in the buffer (may be empty → reader sees 0 = EOF)
            pty.slave_rx.lock().extend_from_slice(&data);
            return;
        }

        // Echo
        if lflag & ECHO != 0 {
            echo_byte(pty, byte);
        } else if lflag & ECHONL != 0 && byte == b'\n' {
            echo_byte(pty, byte);
        }

        // Buffer the byte
        pty.line_buf.push(byte);

        // If newline (or VEOL), flush line buffer to slave_rx
        if byte == b'\n' || (cc[VEOL] != 0 && byte == cc[VEOL])
            || (cc[VEOL2] != 0 && byte == cc[VEOL2])
        {
            let data: Vec<u8> = pty.line_buf.drain(..).collect();
            pty.slave_rx.lock().extend_from_slice(&data);
        }
    } else {
        // --- Raw / non-canonical mode ---
        if lflag & ECHO != 0 {
            echo_byte(pty, byte);
        }
        // Deliver byte immediately to slave
        pty.slave_rx.lock().push(byte);
    }
}

/// Echo a byte back to the master side (so terminal emulator / UART shows it).
fn echo_byte(pty: &mut Pty, byte: u8) {
    // For the console PTY, also write directly to UART so user sees echo
    // immediately without needing the master-side reader to relay.
    if pty.id == console_pty_id() {
        if byte == b'\n' {
            crate::drivers::uart::write_byte(b'\r');
        }
        crate::drivers::uart::write_byte(byte);
    }
    pty.master_rx.lock().push(byte);
}

/// Send a signal to all processes in the given process group.
fn send_signal_to_fg(pgrp: i32, sig: i32) {
    // For now, deliver to all processes matching the process group
    // The process table uses PIDs; pgrp == pid for the group leader.
    // Simple approach: signal all processes with matching tgid/pgid
    crate::ipc::signal::deliver_signal_to_pgrp(pgrp, sig);
}

// ---------------------------------------------------------------------------
// Output processing (slave write → master read path)
// ---------------------------------------------------------------------------

/// Process output from slave through the line discipline (c_oflag).
/// Returns bytes that should be written to master_rx.
pub fn process_output(pty: &Pty, data: &[u8]) -> Vec<u8> {
    let oflag = pty.termios.c_oflag;
    if oflag & OPOST == 0 {
        return data.to_vec();
    }
    let mut out = Vec::with_capacity(data.len() + data.len() / 4);
    for &b in data {
        if oflag & ONLCR != 0 && b == b'\n' {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

// ---------------------------------------------------------------------------
// Master write → line discipline → slave_rx (used for non-console PTYs)
// ---------------------------------------------------------------------------

/// Write data from the master side into the PTY, going through the line
/// discipline before it reaches slave_rx.
pub fn master_write(id: u32, data: &[u8]) {
    if let Some(pty_arc) = get_pty(id) {
        let mut pty = pty_arc.lock();
        for &b in data {
            input_byte_ldisc(&mut pty, b);
        }
    }
}

/// Write data from the slave side, processing through c_oflag, into master_rx.
pub fn slave_write(id: u32, data: &[u8]) {
    if let Some(pty_arc) = get_pty(id) {
        let pty = pty_arc.lock();
        let processed = process_output(&pty, data);
        pty.master_rx.lock().extend_from_slice(&processed);
    }
}
