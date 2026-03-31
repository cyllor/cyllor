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

    #[cfg(target_arch = "aarch64")]
    arch::aarch64::start_secondary_cpus();

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
    let init_paths = ["/bin/bash", "/bin/hello_dyn", "/bin/hello", "/bin/hello_raw"];
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
    // Get user process info
    let (entry, user_sp, our_root_phys) = {
        let sched = sched::SCHEDULER.lock();
        let thread = match sched.run_queues[0].front() {
            Some(t) if t.is_user => t,
            _ => { log::warn!("No user process"); return; }
        };
        (thread.context.elr, thread.context.sp_el0, thread.context.ttbr0)
    };

    let hhdm = arch::aarch64::hhdm_offset();

    // Get Limine's current TTBR0 (which has HHDM mappings)
    let limine_ttbr0: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) limine_ttbr0) };
    let limine_l0_phys = limine_ttbr0 & 0x0000_FFFF_FFFF_F000;

    // Create a NEW L0 table: copy Limine's entries, then overlay ours
    let new_l0_phys = mm::pmm::alloc_page().expect("Failed to alloc page") as u64;
    let new_l0 = (new_l0_phys + hhdm) as *mut u64;

    // Zero it
    unsafe { core::ptr::write_bytes(new_l0, 0, 4096) };

    let limine_l0 = (limine_l0_phys + hhdm) as *const u64;
    let our_l0 = (our_root_phys + hhdm) as *const u64;

    // Copy Limine's entries first
    for i in 0..512 {
        let limine_entry = unsafe { core::ptr::read_volatile(limine_l0.add(i)) };
        if limine_entry != 0 {
            unsafe { core::ptr::write_volatile(new_l0.add(i), limine_entry) };
        }
    }

    drivers::uart::early_print("Limine L0 copied.\n");

    // WORKAROUND: The page table entries for the interpreter (loaded after the
    // main ELF) may not be in our L0 table due to a PMM/paging bug.
    // Re-load interpreter pages directly into the new L0 table.
    {
        let sg = sched::SCHEDULER.lock();
        let t = sg.run_queues[0].front().unwrap();
        if let Some(ref aspace) = t.address_space {
            // Check if interp pages exist in our table
            let our_l0_check = (aspace.root_phys + hhdm) as *const u64;
            let interp_l0 = unsafe { core::ptr::read_volatile(our_l0_check.add(0xA)) };
            if interp_l0 == 0 {
                log::warn!("Interp L0[0xA] missing! Re-mapping interpreter...");
                // Find the interpreter entry by checking the thread context
                let interp_base: u64 = 0x0000_0050_0000_0000;
                // Load the interpreter DIRECTLY into the new merged L0 table
                let interp_path = "/lib/ld-linux-aarch64.so.1";
                if let Ok(node) = fs::vfs::resolve_path(interp_path) {
                    let data = node.lock().data.clone();
                    if !data.is_empty() {
                        // Parse ELF and map segments into new_l0_phys
                        map_interp_to_table(new_l0_phys, &data, interp_base, hhdm);
                    }
                } else if let Ok(data) = fs::ext4::read_file(interp_path) {
                    map_interp_to_table(new_l0_phys, &data, interp_base, hhdm);
                }
            }
        }
        drop(sg);
    }

    // For user VA ranges (covered by our page table), use OUR entries.
    // For HHDM ranges (covered by Limine), keep Limine's entries.
    // Our user pages are at L0 index 0 (VA 0x0-0x7FFFFFFFFF)
    // and stack at L0 index 0xFF (VA 0x7F8000000000-0x7FFFFFFFFFF)
    for i in 0..512 {
        let our_entry = unsafe { core::ptr::read_volatile(our_l0.add(i)) };
        if our_entry != 0 {
            // Use OUR entry — it has our user page mappings
            unsafe { core::ptr::write_volatile(new_l0.add(i), our_entry) };
        }
    }

    drivers::uart::early_print("User L0 merged.\n");

    // Verify the merged mapping
    let verify_l0 = (new_l0_phys + hhdm) as *const u64;
    let l0_entry = unsafe { core::ptr::read_volatile(verify_l0) }; // L0[0]
    drivers::uart::early_print("Merged L0[0]=");
    print_hex_early(l0_entry);
    drivers::uart::early_print("\n");

    // Walk to find what's at 0x400080 via new table
    if l0_entry & 1 != 0 {
        let l1_phys = l0_entry & 0x0000_FFFF_FFFF_F000;
        let l1 = (l1_phys + hhdm) as *const u64;
        let l1_entry = unsafe { core::ptr::read_volatile(l1) }; // L1[0] for VA < 1GB
        drivers::uart::early_print("L1[0]=");
        print_hex_early(l1_entry);
        drivers::uart::early_print("\n");

        if l1_entry & 1 != 0 {
            let l2_phys = l1_entry & 0x0000_FFFF_FFFF_F000;
            let l2 = (l2_phys + hhdm) as *const u64;
            let l2_idx = (0x400080 >> 21) & 0x1FF; // = 2
            let l2_entry = unsafe { core::ptr::read_volatile(l2.add(l2_idx as usize)) };
            drivers::uart::early_print("L2[2]=");
            print_hex_early(l2_entry);
            drivers::uart::early_print("\n");

            if l2_entry & 1 != 0 {
                let l3_phys = l2_entry & 0x0000_FFFF_FFFF_F000;
                let l3 = (l3_phys + hhdm) as *const u64;
                let l3_idx = (0x400080 >> 12) & 0x1FF; // = 0
                let l3_entry = unsafe { core::ptr::read_volatile(l3.add(l3_idx as usize)) };
                drivers::uart::early_print("L3[0]=");
                print_hex_early(l3_entry);
                drivers::uart::early_print("\n");

                if l3_entry & 1 != 0 {
                    let page_phys = l3_entry & 0x0000_FFFF_FFFF_F000;
                    let page_kva = (page_phys + hhdm + 0x80) as *const u32;
                    let inst = unsafe { core::ptr::read_volatile(page_kva) };
                    drivers::uart::early_print("Inst@400080=");
                    print_hex_early(inst as u64);
                    drivers::uart::early_print("\n");
                }
            }
        }
    }

    // Check our original page table L0 entries
    {
        let sg = sched::SCHEDULER.lock();
        let t = sg.run_queues[0].front().unwrap();
        if let Some(ref aspace) = t.address_space {
            log::debug!("Thread root_phys = 0x{:x}", aspace.root_phys);
            let our = (aspace.root_phys + hhdm) as *const u64;
            log::debug!("Our L0[0] = 0x{:x}", unsafe { *our.add(0) });
            log::debug!("Our L0[0xA] = 0x{:x}", unsafe { *our.add(0xA) });
            log::debug!("Our L0[0xFF] = 0x{:x}", unsafe { *our.add(0xFF) });
        }
        drop(sg);
    }

    // Verify key L0 entries in merged table
    let verify_l0 = (new_l0_phys + hhdm) as *const u64;
    log::debug!("Merged L0[0] = 0x{:x}", unsafe { *verify_l0.add(0) });   // code (0x400000)
    log::debug!("Merged L0[0xA] = 0x{:x}", unsafe { *verify_l0.add(0xA) }); // interp (0x5000000000)
    log::debug!("Merged L0[0xFF] = 0x{:x}", unsafe { *verify_l0.add(0xFF) }); // stack

    // Verify stack page is mapped
    let hhdm2 = arch::aarch64::hhdm_offset();
    let merged_l0 = (new_l0_phys + hhdm2) as *const u64;
    let stack_l0_idx = (0x7FFFFFFFEu64 >> 27) & 0x1FF;  // L0 index for stack
    let stack_l0_entry = unsafe { core::ptr::read_volatile(merged_l0.add(0xFF)) };
    log::debug!("Merged L0[0xFF] (stack) = 0x{:x}", stack_l0_entry);

    // Try to read from the stack address via HHDM to verify data
    let sched_guard = sched::SCHEDULER.lock();
    let thread = sched_guard.run_queues[0].front().unwrap();
    if let Some(ref aspace) = thread.address_space {
        if let Some(sp_phys) = aspace.translate(user_sp) {
            let sp_kva = sp_phys + hhdm2;
            let argc = unsafe { *(sp_kva as *const u64) };
            log::debug!("Stack verify: sp=0x{:x} phys=0x{:x} argc={}", user_sp, sp_phys, argc);
            // Read first 24 u64s from sp to see argc+argv+envp+auxv
            for i in 0..24 {
                let val = unsafe { *((sp_kva + i * 8) as *const u64) };
                log::debug!("  sp[{}] = 0x{:x}", i, val);
            }
        } else {
            log::error!("Stack VA 0x{:x} not mapped!", user_sp);
        }
    }
    drop(sched_guard);

    log::info!("ERET to EL0: entry=0x{entry:x} sp=0x{user_sp:x}");

    let spsr: u64 = 0; // EL0t
    unsafe { jump_to_el0(entry, spsr, user_sp, new_l0_phys) };
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

/// Map interpreter ELF segments directly into a page table
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
        while aligned_start + offset < aligned_end {
            let page_va = aligned_start + offset;
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
