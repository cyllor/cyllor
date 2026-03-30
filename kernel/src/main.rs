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

    // Run the first user process directly
    run_first_user_process();

    loop {
        arch::PlatformArch::halt();
    }
}

fn run_first_user_process() {
    // Take the user thread from the scheduler and jump to it directly
    let thread_ctx = {
        let mut sched = sched::SCHEDULER.lock();
        if sched.run_queues[0].is_empty() {
            log::warn!("No user process to run");
            return;
        }
        let mut thread = sched.run_queues[0].pop_front().unwrap();
        if !thread.is_user {
            sched.run_queues[0].push_front(thread);
            return;
        }
        // Set up trampoline registers
        thread.context.x19 = thread.context.elr;     // entry point
        thread.context.x20 = thread.context.spsr;    // SPSR = 0 (EL0t)
        thread.context.x21 = thread.context.sp_el0;  // user SP
        thread.context.x22 = thread.context.ttbr0;   // page table

        thread.state = sched::ThreadState::Running;
        let ctx = thread.context.clone();
        sched.current[0] = Some(thread);
        ctx
    };

    // Verify page table mapping
    {
        let sched_guard = sched::SCHEDULER.lock();
        let current = sched_guard.current[0].as_ref().unwrap();
        if let Some(ref aspace) = current.address_space {
            let phys = aspace.translate(0x400080);
            log::info!("Page table: 0x400080 -> phys {:?}", phys.map(|p| alloc::format!("0x{p:x}")));
            let stack_phys = aspace.translate(0x7ffffffef000);
            log::info!("Page table: stack -> phys {:?}", stack_phys.map(|p| alloc::format!("0x{p:x}")));
        }
        drop(sched_guard);
    }

    // Verify code at entry point
    {
        let sched_guard = sched::SCHEDULER.lock();
        let current = sched_guard.current[0].as_ref().unwrap();
        if let Some(ref aspace) = current.address_space {
            let phys = aspace.translate(0x400080).unwrap();
            let hhdm = arch::aarch64::hhdm_offset();
            let kern_ptr = (phys + hhdm) as *const u32;
            unsafe {
                log::info!("Code at 0x400080: {:08x} {:08x} {:08x} {:08x}",
                    *kern_ptr, *kern_ptr.add(1), *kern_ptr.add(2), *kern_ptr.add(3));
            }
        }
        drop(sched_guard);
    }

    // Verify TCR and TTBR0
    let tcr: u64;
    let ttbr0: u64;
    let sctlr: u64;
    unsafe {
        core::arch::asm!("mrs {}, TCR_EL1", out(reg) tcr);
        core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) ttbr0);
        core::arch::asm!("mrs {}, SCTLR_EL1", out(reg) sctlr);
    }
    log::info!("TCR_EL1=0x{tcr:016x} TTBR0=0x{ttbr0:016x} SCTLR=0x{sctlr:016x}");
    log::info!("Jumping to userspace: entry=0x{:x} sp=0x{:x} ttbr0=0x{:x}", thread_ctx.x19, thread_ctx.x21, thread_ctx.x22);

    // Jump to userspace
    let entry = thread_ctx.x19;
    let spsr: u64 = 0; // EL0t
    let user_sp = thread_ctx.x21;
    let ttbr0 = thread_ctx.x22;

    let current_el: u64;
    unsafe { core::arch::asm!("mrs {}, CurrentEL", out(reg) current_el) };
    log::info!("Current EL: {}", (current_el >> 2) & 3);
    log::info!("ERET to EL0: entry=0x{entry:x} sp=0x{user_sp:x} ttbr0=0x{ttbr0:x}");

    // Use a naked function to do the ERET to avoid compiler interference
    unsafe { jump_to_el0(entry, spsr, user_sp, ttbr0) };
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
        // adr x1, msg (pc + 28 = 0x4000a0)
        0xe1, 0x00, 0x00, 0x10,
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

core::arch::global_asm!(
    ".global jump_to_el0",
    "jump_to_el0:",
    // x0 = entry, x1 = spsr, x2 = user_sp, x3 = ttbr0
    "msr DAIFSet, #0xf",       // Mask interrupts
    "msr SPSel, #0",           // Use SP_EL0 for SP
    "mov sp, x2",             // Set SP (which is now SP_EL0)
    "msr SPSel, #1",          // Switch back to SP_EL1
    "msr TTBR0_EL1, x3",      // Set user page table
    "isb",
    "msr ELR_EL1, x0",        // Entry point
    "msr SPSR_EL1, x1",       // EL0t mode
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
    loop {
        arch::PlatformArch::halt();
    }
}
