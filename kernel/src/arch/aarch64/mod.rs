use crate::arch::Arch;

pub mod exceptions;
pub mod gic;
pub mod timer;

pub struct Aarch64Arch;

static HHDM_OFFSET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

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

pub fn early_init() {
    let hhdm = HHDM_REQUEST
        .response()
        .expect("HHDM response not available");
    HHDM_OFFSET.store(hhdm.offset, core::sync::atomic::Ordering::Relaxed);
    log::debug!("HHDM offset: 0x{:016x}", hhdm.offset);
}

#[allow(dead_code)]
pub fn hhdm_offset() -> u64 {
    HHDM_OFFSET.load(core::sync::atomic::Ordering::Relaxed)
}

pub fn memory_map() -> &'static [&'static limine::memmap::Entry] {
    MEMMAP_REQUEST
        .response()
        .expect("Memory map not available")
        .entries()
}

pub fn cpu_count() -> usize {
    MP_REQUEST
        .response()
        .map(|r| r.cpus().len())
        .unwrap_or(1)
}

impl Arch for Aarch64Arch {
    fn init_interrupts() {
        exceptions::init();
        gic::init_distributor();
        gic::init_cpu_interface();
        timer::init();

        unsafe {
            core::arch::asm!("msr daifclr, #0xf");
        }
    }

    fn enable_interrupts() {
        unsafe {
            core::arch::asm!("msr daifclr, #0xf");
        }
    }

    fn disable_interrupts() {
        unsafe {
            core::arch::asm!("msr daifset, #0xf");
        }
    }

    fn halt() {
        unsafe {
            core::arch::asm!("wfe");
        }
    }
}

/// Start secondary CPUs
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
        // Skip BSP (first CPU or the one with matching MPIDR)
        if i == 0 {
            continue;
        }
        cpu.bootstrap(ap_entry, i as u64);
    }
}

unsafe extern "C" fn ap_entry(info: &limine::mp::MpInfo) -> ! {
    let cpu_id = info.extra_argument() as usize;
    crate::drivers::uart::early_print("AP core starting\n");

    gic::init_cpu_interface();
    timer::init();
    exceptions::init();

    unsafe {
        core::arch::asm!("msr daifclr, #0xf");
    }

    log::info!("CPU {cpu_id} online");

    loop {
        unsafe { core::arch::asm!("wfe") };
        crate::sched::timer_tick();
    }
}
