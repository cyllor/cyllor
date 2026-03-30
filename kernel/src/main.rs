#![no_std]
#![no_main]
#![allow(unused_imports, unused_variables, dead_code)]

extern crate alloc;

mod arch;
mod drivers;
mod fs;
mod ipc;
mod mm;
mod net;
mod sched;
mod sync;
mod syscall;

use arch::Arch;

#[unsafe(no_mangle)]
unsafe extern "C" fn _start() -> ! {
    drivers::uart::early_print("Cyllor: kernel entry reached\n");

    drivers::uart::init();
    log::set_logger(&drivers::uart::LOGGER)
        .map(|()| log::set_max_level(log::LevelFilter::Debug))
        .unwrap();

    log::info!("Cyllor OS booting on {}", arch::ARCH_NAME);

    arch::early_init();
    log::info!("Arch early init done");

    mm::init();
    log::info!("Memory manager initialized");

    fs::init();

    drivers::framebuffer::init();

    arch::PlatformArch::init_interrupts();
    log::info!("Interrupts initialized");

    // Initialize scheduler
    let num_cpus = arch::cpu_count();
    sched::init(num_cpus);

    // Start secondary CPUs
    #[cfg(target_arch = "aarch64")]
    arch::aarch64::start_secondary_cpus();

    // Spawn a test kernel thread
    sched::spawn_kernel_thread("test", test_thread);

    log::info!("Cyllor OS boot complete - {} CPUs", num_cpus);

    loop {
        arch::PlatformArch::halt();
    }
}

fn test_thread() {
    log::info!("Test kernel thread running!");
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    drivers::uart::early_print("KERNEL PANIC: ");
    log::error!("KERNEL PANIC: {info}");
    loop {
        arch::PlatformArch::halt();
    }
}
