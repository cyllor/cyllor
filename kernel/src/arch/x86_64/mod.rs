use crate::arch::Arch;

pub struct X86Arch;

/// Write a byte to COM1 (x86_64 UART).
pub fn uart_write_byte(byte: u8) {
    unsafe {
        core::arch::asm!(
            "2: in al, dx",
            "test al, 0x20",
            "jz 2b",
            "mov al, {0}",
            "mov dx, 0x3F8",
            "out dx, al",
            in(reg_byte) byte,
            out("al") _,
            out("dx") _,
        );
    }
}

/// Initialize COM1 at 115200 baud, 8N1.
pub fn uart_init() {
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3F9", "xor al, al", "out dx, al",   // Disable interrupts
            "mov dx, 0x3FB", "mov al, 0x80", "out dx, al", // DLAB
            "mov dx, 0x3F8", "mov al, 0x01", "out dx, al", // Baud 115200
            "mov dx, 0x3F9", "xor al, al", "out dx, al",
            "mov dx, 0x3FB", "mov al, 0x03", "out dx, al", // 8N1
            "mov dx, 0x3FC", "mov al, 0x03", "out dx, al", // RTS/DSR
            out("al") _,
            out("dx") _,
        );
    }
}

// --------------- Stub types for x86_64 (unimplemented) ---------------

#[derive(Debug, Clone, Copy, Default)]
pub struct TrapFrame;

/// Stub address space — no page table operations implemented on x86_64 yet.
pub struct AddressSpace {
    pub root_phys: u64,
}

impl AddressSpace {
    pub fn new() -> Option<Self> { None }
    pub fn map_page(&self, _virt: u64, _phys: u64, _flags: PageFlags) -> Result<(), ()> { Ok(()) }
    pub fn map_anon(&self, _virt: u64, _size: usize, _flags: PageFlags) -> Result<(), ()> { Ok(()) }
    pub fn translate(&self, _virt: u64) -> Option<u64> { None }
    pub fn copy_to_user(&self, _user_virt: u64, _data: &[u8]) -> Result<(), ()> { Ok(()) }
    pub fn copy_from_user(&self, _user_virt: u64, _buf: &mut [u8]) -> Result<(), ()> { Ok(()) }
    pub fn activate(&self) {}
}

#[derive(Debug, Clone, Copy)]
pub struct PageFlags {
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
    pub user: bool,
    pub device: bool,
}

impl PageFlags {
    pub const USER_RWX: Self = Self { readable: true, writable: true, executable: true, user: true, device: false };
    pub const USER_RW:  Self = Self { readable: true, writable: true, executable: false, user: true, device: false };
    pub const USER_RX:  Self = Self { readable: true, writable: false, executable: true, user: true, device: false };
    pub const USER_RO:  Self = Self { readable: true, writable: false, executable: false, user: true, device: false };
    pub const KERNEL_RW:  Self = Self { readable: true, writable: true, executable: false, user: false, device: false };
    pub const KERNEL_RWX: Self = Self { readable: true, writable: true, executable: true, user: false, device: false };
    pub const DEVICE: Self = Self { readable: true, writable: true, executable: false, user: false, device: true };
}

static HHDM_OFFSET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

#[used]
#[unsafe(link_section = ".requests")]
static HHDM_REQUEST: limine::request::HhdmRequest = limine::request::HhdmRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MEMMAP_REQUEST: limine::request::MemmapRequest = limine::request::MemmapRequest::new();

#[used]
#[unsafe(link_section = ".requests_start_marker")]
static _START_MARKER: limine::RequestsStartMarker = limine::RequestsStartMarker::new();

#[used]
#[unsafe(link_section = ".requests_end_marker")]
static _END_MARKER: limine::RequestsEndMarker = limine::RequestsEndMarker::new();

pub fn early_init() {
    let hhdm = HHDM_REQUEST
        .response()
        .expect("HHDM response not available");
    HHDM_OFFSET.store(hhdm.offset, core::sync::atomic::Ordering::Relaxed);
}

pub fn memory_map() -> &'static [&'static limine::memmap::Entry] {
    MEMMAP_REQUEST
        .response()
        .expect("Memory map not available")
        .entries()
}

impl Arch for X86Arch {
    fn init_interrupts() {
        // Phase 2: IDT + APIC init
    }

    fn enable_interrupts() {
        unsafe {
            core::arch::asm!("sti");
        }
    }

    fn disable_interrupts() {
        unsafe {
            core::arch::asm!("cli");
        }
    }

    fn halt() {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}
