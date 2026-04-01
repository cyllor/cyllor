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

    #[cfg(target_arch = "aarch64")]
    arch::aarch64::paging::init_mair();

    drivers::framebuffer::init();
    arch::PlatformArch::init_interrupts();

    let num_cpus = arch::cpu_count();
    sched::init(num_cpus);

    // NOTE: Don't clear TTBR0 during boot — Limine's boot stack is
    // identity-mapped via TTBR0. TLB flush happens in the trampoline after
    // switch_to_new moves us to the user thread's HHDM kernel stack.

    // Secondary CPUs disabled — GICv2 driver needs work for AP init
    // #[cfg(target_arch = "aarch64")]
    // arch::aarch64::start_secondary_cpus();

    drivers::uart::early_print("A1\n");
    // Create the console PTY (pty 0) — UART RX bytes flow into this PTY
    let console_pty_id = drivers::pty::create_console_pty();
    drivers::uart::early_print("A2\n");
    log::info!("Console PTY: /dev/pts/{console_pty_id}");

    // Enable UART RX interrupt so typed characters reach the PTY
    #[cfg(target_arch = "aarch64")]
    arch::aarch64::gic::enable_irq(arch::aarch64::gic::UART0_IRQ);
    drivers::uart::enable_rx_interrupt();

    // Probe VirtIO devices
    let hhdm = arch::aarch64::hhdm_offset();
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
    let init_paths = ["/bin/bash", "/bin/hello_raw", "/bin/hello"];
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

    run_first_user_process();

    loop { arch::PlatformArch::halt(); }
}

fn run_first_user_process() {
    // All user mappings (ELF, interpreter, stack) are already in aspace.root_phys.
    // No TTBR0 merge with Limine is needed — user code only accesses user VA space,
    // and kernel syscalls use TTBR1 for kernel VA.

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
    // After ERET to EL0, SPSR_EL1=0 restores PSTATE with IRQs enabled.
    unsafe { core::arch::asm!("msr DAIFSet, #2"); }

    crate::sched::scheduler::schedule();

    // Never reached — schedule() → switch_to_new → trampoline → eret to EL0
    loop { arch::PlatformArch::halt(); }
}

/// Minimal static AArch64 ELF that does write(1, "Hello from userspace!\n", 22) then exit(0)
fn create_test_elf() {
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
    elf[18..20].copy_from_slice(&183u16.to_le_bytes()); // EM_AARCH64
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

/// Load specific files from ext4 rootfs into the in-memory VFS on demand
fn load_rootfs_to_vfs() {
    // Only load specific binaries needed for init, not everything
    let critical_files = [
        // Dynamic linker (try both usrmerge and classic paths)
        "/lib/ld-linux-aarch64.so.1",
        "/usr/lib/aarch64-linux-gnu/ld-linux-aarch64.so.1",
        // glibc core
        "/lib/aarch64-linux-gnu/libc.so.6",
        "/usr/lib/aarch64-linux-gnu/libc.so.6",
        "/lib/aarch64-linux-gnu/libm.so.6",
        "/lib/aarch64-linux-gnu/libdl.so.2",
        "/usr/lib/aarch64-linux-gnu/libdl.so.2",
        "/lib/aarch64-linux-gnu/libpthread.so.0",
        "/lib/aarch64-linux-gnu/libgcc_s.so.1",
        "/lib/aarch64-linux-gnu/libtinfo.so.6",
        "/lib/aarch64-linux-gnu/libreadline.so.8",
        // Binaries
        "/sbin/init", "/usr/sbin/init",
        "/bin/bash", "/usr/bin/bash",
        "/bin/sh", "/usr/bin/sh",
        "/bin/hello", "/bin/hello_raw", "/bin/hello_dyn",
        "/usr/bin/weston",
        "/usr/bin/startxfce4",
        "/usr/bin/xfce4-session",
    ];

    for path in &critical_files {
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

        // For ld-linux, also create a copy at /lib/ld-linux-aarch64.so.1
        if path.contains("ld-linux") {
            if let Ok(data) = fs::ext4::read_file(path) {
                let size = data.len();
                if let Ok(lib_node) = fs::vfs::resolve_path("/lib") {
                    let mut ln = lib_node.lock();
                    let mut inode = fs::vfs::Inode::new_file(0o755);
                    inode.data = data;
                    inode.size = size;
                    ln.children.insert(
                        alloc::string::ToString::to_string("ld-linux-aarch64.so.1"),
                        alloc::sync::Arc::new(spin::Mutex::new(inode)),
                    );
                    log::debug!("Linked /lib/ld-linux-aarch64.so.1 ({} KiB)", size / 1024);
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

/// Map interpreter ELF segments directly into a page table (unused, kept for reference)
#[allow(dead_code)]
fn map_interp_to_table(l0_phys: u64, data: &[u8], base: u64, hhdm: u64) {
    use crate::arch::aarch64::paging::PageFlags;
    use crate::mm::mmap::map_page_in_ttbr0;

    if data.len() < 64 { return; }

    let ehdr = unsafe { &*(data.as_ptr() as *const sched::elf::Elf64Ehdr) };
    if ehdr.e_ident[0..4] != [0x7f, b'E', b'L', b'F'] { return; }

    let phdrs = unsafe {
        core::slice::from_raw_parts(
            data.as_ptr().add(ehdr.e_phoff as usize) as *const sched::elf::Elf64Phdr,
            ehdr.e_phnum as usize,
        )
    };

    // Find min vaddr for load bias
    let mut min_vaddr = u64::MAX;
    for phdr in phdrs {
        if phdr.p_type == 1 && phdr.p_vaddr < min_vaddr {
            min_vaddr = phdr.p_vaddr;
        }
    }
    let load_bias = base - (min_vaddr & !0xFFF);

    for phdr in phdrs {
        if phdr.p_type != 1 { continue; } // PT_LOAD only
        let vaddr = phdr.p_vaddr + load_bias;
        let aligned_start = vaddr & !0xFFF;
        let aligned_end = (vaddr + phdr.p_memsz + 0xFFF) & !0xFFF;

        let writable = phdr.p_flags & 2 != 0;
        let executable = phdr.p_flags & 1 != 0;
        let flags = PageFlags {
            readable: true, writable, executable, user: true, device: false,
        };

        let mut offset = 0u64;
        let mut first = true;
        while aligned_start + offset < aligned_end {
            let page_va = aligned_start + offset;
            if first {
                drivers::uart::early_print("map_interp va=");
                print_hex_early(page_va);
                drivers::uart::early_print(" l0=");
                print_hex_early(l0_phys);
                drivers::uart::early_print("\n");
                first = false;
            }
            let phys = mm::pmm::alloc_page().unwrap() as u64;
            unsafe { core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, 4096); }

            // Copy file data for this page
            if page_va >= vaddr && phdr.p_filesz > 0 {
                let offset_in_seg = (page_va - vaddr) as usize;
                if offset_in_seg < phdr.p_filesz as usize {
                    let file_start = phdr.p_offset as usize + offset_in_seg;
                    let remaining = (phdr.p_filesz as usize) - offset_in_seg;
                    let copy_len = remaining.min(4096);
                    if file_start + copy_len <= data.len() {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                data[file_start..].as_ptr(),
                                (phys + hhdm) as *mut u8,
                                copy_len,
                            );
                        }
                    }
                }
            } else if page_va < vaddr && page_va + 4096 > vaddr && phdr.p_filesz > 0 {
                // Page straddles the segment start
                let skip = (vaddr - page_va) as usize;
                let copy_len = (4096 - skip).min(phdr.p_filesz as usize);
                let file_start = phdr.p_offset as usize;
                if file_start + copy_len <= data.len() {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            data[file_start..].as_ptr(),
                            (phys + hhdm + skip as u64) as *mut u8,
                            copy_len,
                        );
                    }
                }
            }

            map_page_in_ttbr0(l0_phys, page_va, phys, flags, hhdm);
            offset += 4096;
        }
    }
    log::info!("Interpreter re-mapped into merged page table");
}

fn print_hex_early(val: u64) {
    for i in (0..16).rev() {
        let nibble = ((val >> (i * 4)) & 0xF) as u8;
        let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
        drivers::uart::write_byte(c);
    }
}

core::arch::global_asm!(
    ".global jump_to_el0",
    "jump_to_el0:",
    // x0=entry, x1=spsr, x2=user_sp, x3=ttbr0
    "msr DAIFSet, #0xf",
    "msr TTBR0_EL1, x3",
    "isb",
    "msr SPSel, #0",
    "mov sp, x2",
    "msr SPSel, #1",
    "msr ELR_EL1, x0",
    "msr SPSR_EL1, x1",
    "isb",
    "mov x0, xzr",
    "mov x1, xzr",
    "mov x2, xzr",
    "mov x3, xzr",
    "mov x4, xzr",
    "mov x5, xzr",
    "mov x6, xzr",
    "mov x7, xzr",
    "mov x8, xzr",
    "eret",
);

unsafe extern "C" {
    fn jump_to_el0(entry: u64, spsr: u64, user_sp: u64, ttbr0: u64) -> !;
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    drivers::uart::early_print("KERNEL PANIC: ");
    log::error!("KERNEL PANIC: {info}");
    loop { arch::PlatformArch::halt(); }
}
