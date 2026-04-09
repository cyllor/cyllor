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
    mm::init();
    fs::init();

    arch::init_mair();

    drivers::framebuffer::init();
    arch::PlatformArch::init_interrupts();

    let num_cpus = arch::cpu_count();
    sched::init(num_cpus);

    // NOTE: Don't clear the user page table during boot — Limine's boot stack
    // is identity-mapped via the user-space page table. TLB flush happens in
    // the trampoline after switch_to_new moves us to the kernel stack.

    arch::start_secondary_cpus();

    drivers::uart::early_print("A1\n");
    // Create the console PTY (pty 0) — UART RX bytes flow into this PTY
    let console_pty_id = drivers::pty::create_console_pty();
    drivers::uart::early_print("A2\n");
    log::info!("Console PTY: /dev/pts/{console_pty_id}");

    // Enable UART RX interrupt so typed characters reach the PTY
    arch::enable_uart_irq();
    drivers::uart::enable_rx_interrupt();

    // Probe VirtIO devices
    let hhdm = arch::hhdm_offset();
    drivers::virtio::block::probe(hhdm);

    // Test disk read
    {
        let mut test_buf = [0u8; 1024];
        match drivers::virtio::block::read_sectors(2, 2, &mut test_buf) {
            Ok(()) => {
                log::info!("Disk read OK: magic=0x{:04x}", u16::from_le_bytes([test_buf[0x38], test_buf[0x39]]));
            }
            Err(e) => log::error!("Disk read failed: {e}"),
        }
    }

    // Try to mount ext4 from VirtIO block device
    match fs::ext4::mount() {
        Ok(()) => {
            log::info!("ext4 root filesystem mounted");
            // Removed verbose debug output
            // Load ext4 files into VFS
            load_rootfs_to_vfs();
        }
        Err(e) => {
            log::warn!("ext4 mount failed: {e} (using test ELF)");
            create_test_elf();
        }
    }

    // Create test ELF only if /bin/hello doesn't exist from rootfs
    if fs::vfs::resolve_path("/bin/hello").is_err() {
        create_test_elf();
    }

    // Spawn init process
    let init_paths = [
        "/usr/bin/startxfce4",
        "/usr/bin/xfce4-session",
        "/usr/bin/weston",
        "/bin/bash",
        "/bin/hello_raw",
        "/bin/hello",
    ];
    for path in &init_paths {
        match sched::spawn_user_process(path, &[path.as_bytes()], &[
            b"PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            b"HOME=/root",
            b"TERM=linux",
            b"DISPLAY=:0",
        ]) {
            Ok(pid) => {
                log::info!("Init process: '{}' PID {pid}", path);
                break;
            }
            Err(_) => continue,
        }
    }

    log::info!("Cyllor OS boot complete - {} CPUs", num_cpus);

    // Wait ~2s for AP timer ticks to accumulate, then report
    log::info!("Waiting 2s for per-CPU tick verification...");
    for _ in 0..20u32 {
        arch::PlatformArch::halt(); // ~100ms per tick
    }
    log::info!("Per-CPU tick report:");
    arch::dump_per_cpu_ticks();

    run_first_user_process();
    loop { arch::PlatformArch::halt(); }
}

fn run_first_user_process() {
    // All user mappings (ELF, interpreter, stack) are already in aspace.root_phys.
    // No merge with Limine's page table is needed — user code only accesses user VA
    // space, and kernel syscalls use kernel-space page tables for kernel VA.

    let has_user = {
        let sched = sched::SCHEDULER.lock();
        sched.run_queues[0].front().map_or(false, |t| t.is_user)
    };

    if !has_user {
        log::warn!("No user process in run queue");
        return;
    }

    log::info!("Starting first user process via scheduler");

    // Mask IRQs to prevent re-entrant SCHEDULER lock deadlock.
    // After return to user mode, IRQs are re-enabled by the arch trampoline.
    arch::mask_irqs();

    crate::sched::scheduler::schedule();

    // Never reached — schedule() → switch_to_new → trampoline → return to user mode
    loop { arch::PlatformArch::halt(); }
}

/// Create a minimal test ELF binary with arch-specific machine code.
/// Currently only implemented for AArch64 (ARM instructions).
#[cfg(target_arch = "aarch64")]
fn create_test_elf() {
    // AArch64 machine code: write(1, "Hello from userspace!\n", 22) then exit(0)
    let code: &[u8] = &[
        0x20, 0x00, 0x80, 0xd2, // mov x0, #1
        0xe1, 0x00, 0x00, 0x10, // adr x1, .+28 (msg at 0x4000a0)
        0xc2, 0x02, 0x80, 0xd2, // mov x2, #22
        0x08, 0x08, 0x80, 0xd2, // mov x8, #64 (__NR_write)
        0x01, 0x00, 0x00, 0xd4, // svc #0
        0x00, 0x00, 0x80, 0xd2, // mov x0, #0
        0xa8, 0x0b, 0x80, 0xd2, // mov x8, #93 (__NR_exit)
        0x01, 0x00, 0x00, 0xd4, // svc #0
        b'H', b'e', b'l', b'l', b'o', b' ', b'f', b'r',
        b'o', b'm', b' ', b'u', b's', b'e', b'r', b's',
        b'p', b'a', b'c', b'e', b'!', b'\n',
    ];

    let mut elf = alloc::vec![0u8; 4096];
    let entry_vaddr: u64 = 0x400080;

    // ELF header
    elf[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    elf[4] = 2; elf[5] = 1; elf[6] = 1;
    elf[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    elf[18..20].copy_from_slice(&arch::ELF_MACHINE.to_le_bytes());
    elf[20..24].copy_from_slice(&1u32.to_le_bytes());
    elf[24..32].copy_from_slice(&entry_vaddr.to_le_bytes());
    elf[32..40].copy_from_slice(&64u64.to_le_bytes()); // phoff
    elf[52..54].copy_from_slice(&64u16.to_le_bytes()); // ehsize
    elf[54..56].copy_from_slice(&56u16.to_le_bytes()); // phentsize
    elf[56..58].copy_from_slice(&1u16.to_le_bytes()); // phnum

    // Program header (PT_LOAD, RWX)
    let ph = 64;
    elf[ph..ph+4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    elf[ph+4..ph+8].copy_from_slice(&7u32.to_le_bytes()); // PF_R|PF_W|PF_X
    elf[ph+16..ph+24].copy_from_slice(&0x400000u64.to_le_bytes()); // vaddr
    elf[ph+24..ph+32].copy_from_slice(&0x400000u64.to_le_bytes()); // paddr
    elf[ph+32..ph+40].copy_from_slice(&4096u64.to_le_bytes()); // filesz
    elf[ph+40..ph+48].copy_from_slice(&4096u64.to_le_bytes()); // memsz
    elf[ph+48..ph+56].copy_from_slice(&0x1000u64.to_le_bytes()); // align

    elf[0x80..0x80 + code.len()].copy_from_slice(code);

    // Add to VFS
    let root = fs::vfs::root();
    let root_node = root.lock();
    if let Some(bin) = root_node.children.get("bin") {
        let mut bin_node = bin.lock();
        let mut hello = fs::vfs::Inode::new_file(0o755);
        hello.data = elf;
        hello.size = hello.data.len();
        bin_node.children.insert(
            alloc::string::ToString::to_string("hello"),
            alloc::sync::Arc::new(spin::Mutex::new(hello)),
        );
    }
    log::info!("Test ELF /bin/hello created");
}

#[cfg(not(target_arch = "aarch64"))]
fn create_test_elf() {
    log::warn!("Test ELF not available for {} (no embedded machine code)", arch::ARCH_NAME);
}

/// Load specific files from ext4 rootfs into the in-memory VFS on demand
fn load_rootfs_to_vfs() {
    let interp = arch::INTERP_NAME;
    let gnu_dir = arch::GNU_LIB_DIR;

    // Build arch-dependent paths at runtime using arch constants.
    let arch_paths: alloc::vec::Vec<alloc::string::String> = alloc::vec![
        alloc::format!("/lib/{}", interp),
        alloc::format!("/usr/lib/{}/{}", gnu_dir, interp),
        alloc::format!("/lib/{}/libc.so.6", gnu_dir),
        alloc::format!("/usr/lib/{}/libc.so.6", gnu_dir),
        alloc::format!("/lib/{}/libm.so.6", gnu_dir),
        alloc::format!("/lib/{}/libdl.so.2", gnu_dir),
        alloc::format!("/usr/lib/{}/libdl.so.2", gnu_dir),
        alloc::format!("/lib/{}/libpthread.so.0", gnu_dir),
        alloc::format!("/lib/{}/libgcc_s.so.1", gnu_dir),
        alloc::format!("/lib/{}/libtinfo.so.6", gnu_dir),
        alloc::format!("/lib/{}/libreadline.so.8", gnu_dir),
    ];

    // Architecture-independent binary paths.
    let static_paths: &[&str] = &[
        "/sbin/init", "/usr/sbin/init",
        "/bin/bash", "/usr/bin/bash",
        "/bin/sh", "/usr/bin/sh",
        "/bin/hello", "/bin/hello_raw", "/bin/hello_dyn",
        "/usr/bin/weston",
        "/usr/bin/startxfce4",
        "/usr/bin/xfce4-session",
    ];

    let all_paths = arch_paths.iter().map(|s| s.as_str())
        .chain(static_paths.iter().copied());

    for path in all_paths {
        // Ensure parent directories exist
        if let Some(parent_path) = path.rsplit_once('/') {
            let dir = parent_path.0;
            ensure_vfs_dir(dir);
        }

        match fs::ext4::read_file(path) {
            Ok(data) => {
                let (dir, name) = path.rsplit_once('/').unwrap();
                if let Ok(parent_node) = fs::vfs::resolve_path(dir) {
                    let mut pn = parent_node.lock();
                    let mut inode = fs::vfs::Inode::new_file(0o755);
                    let size = data.len();
                    inode.data = data;
                    inode.size = size;
                    let node = alloc::sync::Arc::new(spin::Mutex::new(inode));
                    pn.children.insert(
                        alloc::string::ToString::to_string(name),
                        node,
                    );
                    log::debug!("Loaded {} ({} KiB)", path, size / 1024);
                }
            }
            Err(_) => {}
        }

        // For the dynamic linker, also ensure a canonical copy exists at /lib/<interp>
        if path.contains(interp) {
            if let Ok(data) = fs::ext4::read_file(path) {
                let size = data.len();
                if let Ok(lib_node) = fs::vfs::resolve_path("/lib") {
                    let mut ln = lib_node.lock();
                    let mut inode = fs::vfs::Inode::new_file(0o755);
                    inode.data = data;
                    inode.size = size;
                    ln.children.insert(
                        alloc::string::ToString::to_string(interp),
                        alloc::sync::Arc::new(spin::Mutex::new(inode)),
                    );
                    log::debug!("Linked /lib/{} ({} KiB)", interp, size / 1024);
                }
            }
        }
    }
}

fn ensure_vfs_dir(path: &str) {
    let parts: alloc::vec::Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut current = alloc::string::String::new();
    for part in parts {
        current.push('/');
        current.push_str(part);
        let _ = fs::mkdirat(-1, &current, 0o755);
    }
}


fn print_hex_early(val: u64) {
    for i in (0..16).rev() {
        let nibble = ((val >> (i * 4)) & 0xF) as u8;
        let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
        drivers::uart::write_byte(c);
    }
}


#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    drivers::uart::early_print("KERNEL PANIC: ");
    log::error!("KERNEL PANIC: {info}");
    loop { arch::PlatformArch::halt(); }
}
