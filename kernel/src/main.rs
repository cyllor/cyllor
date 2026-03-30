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

    create_test_elf();

    match sched::spawn_user_process("/bin/hello", &[b"hello"], &[b"PATH=/bin"]) {
        Ok(pid) => log::info!("User process spawned: PID {pid}"),
        Err(e) => log::error!("Failed to spawn: {e}"),
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
