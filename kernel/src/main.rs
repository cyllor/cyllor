#![no_std]
#![no_main]
#![allow(unused_imports, unused_variables, dead_code)]
#![feature(naked_functions)]

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

    // Initialize paging (MAIR, TCR)
    #[cfg(target_arch = "aarch64")]
    arch::aarch64::paging::init_mair();

    drivers::framebuffer::init();

    arch::PlatformArch::init_interrupts();
    log::info!("Interrupts initialized");

    // Initialize scheduler
    let num_cpus = arch::cpu_count();
    sched::init(num_cpus);

    // Start secondary CPUs
    #[cfg(target_arch = "aarch64")]
    arch::aarch64::start_secondary_cpus();

    // Create a minimal test ELF in VFS for testing
    create_test_elf();

    // Try to spawn the test user process
    match sched::spawn_user_process("/bin/hello", &[b"hello"], &[b"PATH=/bin"]) {
        Ok(pid) => log::info!("User process spawned: PID {pid}"),
        Err(e) => log::error!("Failed to spawn user process: {e}"),
    }

    log::info!("Cyllor OS boot complete - {} CPUs", num_cpus);

    loop {
        arch::PlatformArch::halt();
    }
}

/// Create a minimal static AArch64 ELF that writes "Hello from userspace!\n" to fd 1
fn create_test_elf() {
    // Minimal AArch64 ELF that does:
    //   mov x0, #1        // fd = stdout
    //   adr x1, msg       // buf
    //   mov x2, #22       // len
    //   mov x8, #64       // __NR_write
    //   svc #0
    //   mov x0, #0        // status = 0
    //   mov x8, #93       // __NR_exit
    //   svc #0
    // msg: "Hello from userspace!\n"

    let code: &[u8] = &[
        // mov x0, #1
        0x20, 0x00, 0x80, 0xd2,
        // adr x1, msg (pc + 24 = 0x400098)
        0xc1, 0x00, 0x00, 0x10,
        // mov x2, #22
        0xc2, 0x02, 0x80, 0xd2,
        // mov x8, #64 (__NR_write for aarch64)
        0x08, 0x08, 0x80, 0xd2,
        // svc #0
        0x01, 0x00, 0x00, 0xd4,
        // mov x0, #0
        0x00, 0x00, 0x80, 0xd2,
        // mov x8, #93 (__NR_exit)
        0xa8, 0x0b, 0x80, 0xd2,
        // svc #0
        0x01, 0x00, 0x00, 0xd4,
        // msg: "Hello from userspace!\n"
        b'H', b'e', b'l', b'l', b'o', b' ', b'f', b'r',
        b'o', b'm', b' ', b'u', b's', b'e', b'r', b's',
        b'p', b'a', b'c', b'e', b'!', b'\n',
    ];

    // Build a minimal ELF
    let mut elf = alloc::vec![0u8; 4096];
    let entry: u64 = 0x400080;

    // ELF header (64 bytes)
    elf[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']); // magic
    elf[4] = 2;  // 64-bit
    elf[5] = 1;  // little endian
    elf[6] = 1;  // ELF version
    elf[7] = 0;  // OS/ABI
    elf[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    elf[18..20].copy_from_slice(&183u16.to_le_bytes()); // EM_AARCH64
    elf[20..24].copy_from_slice(&1u32.to_le_bytes()); // version
    elf[24..32].copy_from_slice(&entry.to_le_bytes()); // entry point
    elf[32..40].copy_from_slice(&64u64.to_le_bytes()); // phoff
    elf[40..48].copy_from_slice(&0u64.to_le_bytes()); // shoff
    elf[48..52].copy_from_slice(&0u32.to_le_bytes()); // flags
    elf[52..54].copy_from_slice(&64u16.to_le_bytes()); // ehsize
    elf[54..56].copy_from_slice(&56u16.to_le_bytes()); // phentsize
    elf[56..58].copy_from_slice(&1u16.to_le_bytes()); // phnum
    elf[58..60].copy_from_slice(&0u16.to_le_bytes()); // shentsize
    elf[60..62].copy_from_slice(&0u16.to_le_bytes()); // shnum

    // Program header (56 bytes at offset 64)
    let ph_offset = 64;
    elf[ph_offset..ph_offset+4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    elf[ph_offset+4..ph_offset+8].copy_from_slice(&7u32.to_le_bytes()); // PF_R|PF_W|PF_X
    elf[ph_offset+8..ph_offset+16].copy_from_slice(&0u64.to_le_bytes()); // offset in file
    elf[ph_offset+16..ph_offset+24].copy_from_slice(&0x400000u64.to_le_bytes()); // vaddr
    elf[ph_offset+24..ph_offset+32].copy_from_slice(&0x400000u64.to_le_bytes()); // paddr
    let total_size = (120 + code.len() - 32) as u64; // ehdr + phdr + code
    elf[ph_offset+32..ph_offset+40].copy_from_slice(&4096u64.to_le_bytes()); // filesz
    elf[ph_offset+40..ph_offset+48].copy_from_slice(&4096u64.to_le_bytes()); // memsz
    elf[ph_offset+48..ph_offset+56].copy_from_slice(&0x1000u64.to_le_bytes()); // align

    // Code at offset 0x80 (= 128) -> maps to vaddr 0x400080
    let code_offset = 0x80;
    elf[code_offset..code_offset + code.len()].copy_from_slice(code);

    // Write to VFS as /bin/hello
    let root = fs::vfs::root();
    let root_node = root.lock();
    if let Some(bin) = root_node.children.get("bin") {
        let mut bin_node = bin.lock();
        let mut hello = fs::vfs::Inode::new_file(0o755);
        hello.data = elf;
        hello.size = hello.data.len();
        bin_node.children.insert(alloc::string::ToString::to_string("hello"), alloc::sync::Arc::new(spin::Mutex::new(hello)));
    }
    log::info!("Test ELF /bin/hello created");
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    drivers::uart::early_print("KERNEL PANIC: ");
    log::error!("KERNEL PANIC: {info}");
    loop {
        arch::PlatformArch::halt();
    }
}
