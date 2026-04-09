use crate::arch::{Arch, CpuContext, PageAttr, PageTable};

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
pub struct TrapFrame {
    pub regs: [u64; 32],
    pub rip: u64,
    pub rsp: u64,
}

impl CpuContext for TrapFrame {
    #[inline]
    fn reg(&self, idx: usize) -> u64 {
        self.regs.get(idx).copied().unwrap_or(0)
    }

    #[inline]
    fn set_reg(&mut self, idx: usize, val: u64) {
        if let Some(slot) = self.regs.get_mut(idx) {
            *slot = val;
        }
    }

    #[inline]
    fn pc(&self) -> u64 { self.rip }

    #[inline]
    fn set_pc(&mut self, val: u64) { self.rip = val; }

    #[inline]
    fn sp(&self) -> u64 { self.rsp }

    #[inline]
    fn set_sp(&mut self, val: u64) { self.rsp = val; }
}

/// Stub address space — no page table operations implemented on x86_64 yet.
pub struct AddressSpace {
    pub root_phys: u64,
}

impl AddressSpace {
    pub fn new() -> Option<Self> { None }
    pub fn map_page(&self, _virt: u64, _phys: u64, _flags: PageAttr) -> Result<(), ()> { Ok(()) }
    pub fn map_anon(&self, _virt: u64, _size: usize, _flags: PageAttr) -> Result<(), ()> { Ok(()) }
    pub fn translate(&self, _virt: u64) -> Option<u64> { None }
    pub fn copy_to_user(&self, _user_virt: u64, _data: &[u8]) -> Result<(), ()> { Ok(()) }
    pub fn copy_from_user(&self, _user_virt: u64, _buf: &mut [u8]) -> Result<(), ()> { Ok(()) }
    pub fn activate(&self) {}
}

impl PageTable for AddressSpace {
    fn root_phys(&self) -> u64 { self.root_phys }
    fn map_anon(&self, virt_start: u64, size: usize, flags: PageAttr) -> Result<(), ()> {
        AddressSpace::map_anon(self, virt_start, size, flags)
    }
    fn copy_to_user(&self, virt: u64, data: &[u8]) -> Result<(), ()> {
        AddressSpace::copy_to_user(self, virt, data)
    }
    fn copy_from_user(&self, virt: u64, buf: &mut [u8]) -> Result<(), ()> {
        AddressSpace::copy_from_user(self, virt, buf)
    }
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
