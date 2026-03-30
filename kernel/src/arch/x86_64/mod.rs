use crate::arch::Arch;

pub struct X86Arch;

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
