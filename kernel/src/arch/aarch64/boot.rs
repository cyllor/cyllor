// arch/aarch64/boot.rs — Limine boot protocol requests and early init for AArch64
use core::sync::atomic::{AtomicU64, Ordering};

static HHDM_OFFSET: AtomicU64 = AtomicU64::new(0);

#[used]
#[unsafe(link_section = ".requests")]
static HHDM_REQUEST: limine::request::HhdmRequest = limine::request::HhdmRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MEMMAP_REQUEST: limine::request::MemmapRequest = limine::request::MemmapRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MP_REQUEST: limine::request::MpRequest = limine::request::MpRequest::new(0);

#[used]
#[unsafe(link_section = ".requests_start_marker")]
static _START_MARKER: limine::RequestsStartMarker = limine::RequestsStartMarker::new();

#[used]
#[unsafe(link_section = ".requests_end_marker")]
static _END_MARKER: limine::RequestsEndMarker = limine::RequestsEndMarker::new();

/// Called once by the BSP before any other kernel init.
pub fn early_init() {
    let hhdm = HHDM_REQUEST
        .response()
        .expect("HHDM response not available");
    HHDM_OFFSET.store(hhdm.offset, Ordering::Relaxed);
    log::debug!("HHDM offset: 0x{:016x}", hhdm.offset);
}

/// Returns the HHDM base offset (physical → virtual translation for the kernel).
#[allow(dead_code)]
pub fn hhdm_offset() -> u64 {
    HHDM_OFFSET.load(Ordering::Relaxed)
}

/// Returns the Limine memory map entries.
pub fn memory_map() -> &'static [&'static limine::memmap::Entry] {
    MEMMAP_REQUEST
        .response()
        .expect("Memory map not available")
        .entries()
}

/// Returns the number of CPUs reported by Limine SMP.
pub fn cpu_count() -> usize {
    MP_REQUEST
        .response()
        .map(|r| r.cpus().len())
        .unwrap_or(1)
}

/// Bring up all secondary (AP) CPUs via Limine SMP protocol.
pub fn start_secondary_cpus() {
    let mp = match MP_REQUEST.response() {
        Some(r) => r,
        None => {
            log::warn!("SMP not available");
            return;
        }
    };

    let cpus = mp.cpus();
    log::info!("SMP: {} CPUs detected", cpus.len());

    for (i, cpu) in cpus.iter().enumerate() {
        if i == 0 {
            continue; // skip BSP
        }
        cpu.bootstrap(ap_entry, i as u64);
    }
}

unsafe extern "C" fn ap_entry(info: &limine::mp::MpInfo) -> ! {
    let cpu_id = info.extra_argument() as usize;
    crate::drivers::uart::early_print("AP core starting\n");

    super::exceptions::init();
    super::gic::init_redistributor();
    super::gic::init_cpu_interface();
    super::timer::init();

    unsafe {
        core::arch::asm!("msr daifclr, #0xf");
    }

    log::info!("CPU {cpu_id} online");

    // Idle loop — timer IRQ handles scheduling
    loop {
        unsafe { core::arch::asm!("wfe") };
    }
}
