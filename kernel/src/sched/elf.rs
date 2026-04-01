// ELF64 loader for Linux binaries

use alloc::string::String;
use alloc::vec::Vec;
use goblin::elf64::header::{Header, ET_DYN};
use goblin::elf64::program_header::{ProgramHeader, PT_LOAD, PT_INTERP, PT_PHDR, PF_X, PF_W, PF_R};
use crate::arch::{AddressSpace, PageFlags};

const PAGE_SIZE: usize = 4096;
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

// Auxiliary vector types
const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_BASE: u64 = 7;
const AT_ENTRY: u64 = 9;
const AT_UID: u64 = 11;
const AT_GID: u64 = 13;
const AT_RANDOM: u64 = 25;

pub struct ElfLoadResult {
    pub entry: u64,           // Entry point (might be interp's entry)
    pub interp_base: u64,     // Base address of interpreter (0 if static)
    pub phdr_addr: u64,       // Address of program headers in memory
    pub phent: u16,           // Size of each program header
    pub phnum: u16,           // Number of program headers
    pub exe_entry: u64,       // Original executable entry point
    pub stack_top: u64,       // Top of user stack
    pub brk_start: u64,       // Start of data segment (for brk)
}

/// Load an ELF binary into the given address space
pub fn load_elf(data: &[u8], aspace: &AddressSpace) -> Result<ElfLoadResult, &'static str> {
    if data.len() < core::mem::size_of::<Header>() {
        return Err("ELF too small");
    }
    let ehdr = unsafe { &*(data.as_ptr() as *const Header) };
    if ehdr.e_ident[0..4] != ELF_MAGIC { return Err("Bad ELF magic"); }
    if ehdr.e_ident[4] != 2 { return Err("Not ELF64"); }
    if ehdr.e_machine != crate::arch::ELF_MACHINE { return Err("Wrong architecture"); }

    let is_pie = ehdr.e_type == ET_DYN as u16;
    let load_bias: u64 = if is_pie { 0x0000_0040_0000 } else { 0 };

    let phdrs = unsafe {
        core::slice::from_raw_parts(
            data.as_ptr().add(ehdr.e_phoff as usize) as *const ProgramHeader,
            ehdr.e_phnum as usize,
        )
    };

    // Find interpreter path (PT_INTERP)
    let mut interp_path: Option<String> = None;
    for phdr in phdrs {
        if phdr.p_type == PT_INTERP {
            let start = phdr.p_offset as usize;
            let end = start + phdr.p_filesz as usize;
            if end <= data.len() {
                let path_bytes = &data[start..end];
                let len = path_bytes.iter().position(|&b| b == 0).unwrap_or(path_bytes.len());
                if let Ok(s) = core::str::from_utf8(&path_bytes[..len]) {
                    interp_path = Some(String::from(s));
                }
            }
        }
    }

    // Load PT_LOAD segments
    let mut brk_end: u64 = 0;
    let mut phdr_addr: u64 = 0;

    for phdr in phdrs {
        if phdr.p_type == PT_LOAD {
            let vaddr = phdr.p_vaddr + load_bias;
            let aligned_start = vaddr & !(PAGE_SIZE as u64 - 1);
            let aligned_end = (vaddr + phdr.p_memsz + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
            let map_size = (aligned_end - aligned_start) as usize;

            let flags = phdr_to_pageflags(phdr.p_flags);
            aspace.map_anon(aligned_start, map_size, flags)
                .map_err(|_| "Failed to map segment")?;

            if phdr.p_filesz > 0 {
                let file_data = &data[phdr.p_offset as usize..(phdr.p_offset + phdr.p_filesz) as usize];
                aspace.copy_to_user(vaddr, file_data)
                    .map_err(|_| "Failed to copy segment data")?;
            }

            let seg_end = vaddr + phdr.p_memsz;
            if seg_end > brk_end { brk_end = seg_end; }
        }
        if phdr.p_type == PT_PHDR {
            phdr_addr = phdr.p_vaddr + load_bias;
        }
    }

    if phdr_addr == 0 {
        phdr_addr = load_bias + ehdr.e_phoff;
    }

    let brk_start = (brk_end + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);

    // Map user stack (8 MiB)
    let stack_size: usize = 8 * 1024 * 1024;
    let stack_top: u64 = crate::arch::USER_STACK_TOP;
    let stack_bottom = stack_top - stack_size as u64;
    aspace.map_anon(stack_bottom, stack_size, PageFlags::USER_RW)
        .map_err(|_| "Failed to map stack")?;

    // Load interpreter if needed
    let mut interp_base: u64 = 0;
    let mut final_entry = ehdr.e_entry + load_bias;

    if let Some(ref path) = interp_path {
        log::debug!("ELF interpreter: {}", path);
        match crate::fs::vfs::resolve_path(path) {
            Ok(node) => {
                let interp_data = node.lock().data.clone();
                if !interp_data.is_empty() {
                    let ld_base: u64 = 0x0000_0050_0000_0000;
                    let ld_entry = load_interp(&interp_data, aspace, ld_base)?;
                    interp_base = ld_base;
                    final_entry = ld_entry;
                    let h = crate::arch::hhdm_offset();
                    let l0p = (aspace.root_phys + h) as *const u64;
                    let l0a = unsafe { core::ptr::read_volatile(l0p.add(0xA)) };
                    log::info!("VFS interp: L0[0xA]=0x{:x} entry=0x{:x}", l0a, ld_entry);
                }
            }
            Err(_) => {
                match crate::fs::ext4::read_file(path) {
                    Ok(interp_data) => {
                        let ld_base: u64 = 0x0000_0050_0000_0000;
                        let ld_entry = load_interp(&interp_data, aspace, ld_base)?;
                        interp_base = ld_base;
                        final_entry = ld_entry;
                        let hhdm_off = crate::arch::hhdm_offset();
                        let l0 = (aspace.root_phys + hhdm_off) as *const u64;
                        let l0_a = unsafe { core::ptr::read_volatile(l0.add(0xA)) };
                        log::info!("After load_interp: L0[0xA] = 0x{:x}", l0_a);
                        log::info!("Loaded interpreter at 0x{:x}, entry=0x{:x}", ld_base, ld_entry);
                    }
                    Err(_) => {
                        log::warn!("Interpreter {} not found, trying static", path);
                    }
                }
            }
        }
    }

    log::debug!("ELF load: entry=0x{:x} phdr=0x{:x} phent={} phnum={} interp_base=0x{:x} brk=0x{:x} bias=0x{:x}",
        final_entry, phdr_addr, ehdr.e_phentsize, ehdr.e_phnum, interp_base, brk_start, load_bias);

    Ok(ElfLoadResult {
        entry: final_entry,
        interp_base,
        phdr_addr,
        phent: ehdr.e_phentsize,
        phnum: ehdr.e_phnum,
        exe_entry: ehdr.e_entry + load_bias,
        stack_top,
        brk_start,
    })
}

/// Build the initial user stack with argv, envp, and auxval.
/// Returns the new stack pointer.
pub fn setup_user_stack(
    aspace: &AddressSpace,
    stack_top: u64,
    argv: &[&[u8]],
    envp: &[&[u8]],
    elf_result: &ElfLoadResult,
) -> Result<u64, &'static str> {
    let mut sp = stack_top;

    let push_bytes = |sp: &mut u64, data: &[u8], aspace: &AddressSpace| -> Result<u64, &'static str> {
        *sp -= data.len() as u64;
        aspace.copy_to_user(*sp, data).map_err(|_| "stack write failed")?;
        Ok(*sp)
    };

    let push_u64 = |sp: &mut u64, val: u64, aspace: &AddressSpace| -> Result<(), &'static str> {
        *sp -= 8;
        aspace.copy_to_user(*sp, &val.to_le_bytes()).map_err(|_| "stack write failed")?;
        Ok(())
    };

    sp &= !0xF;

    // AT_RANDOM: 16 bytes of pseudo-random data
    sp -= 16;
    let random_addr = sp;
    let mut random_data = [0u8; 16];
    let mut seed: u64 = crate::arch::read_counter();
    for b in random_data.iter_mut() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        *b = (seed >> 33) as u8;
    }
    aspace.copy_to_user(random_addr, &random_data).map_err(|_| "stack write")?;

    let mut env_ptrs = Vec::new();
    for env in envp.iter().rev() {
        sp -= env.len() as u64 + 1;
        sp &= !0x7;
        let mut buf = env.to_vec();
        buf.push(0);
        aspace.copy_to_user(sp, &buf).map_err(|_| "env write")?;
        env_ptrs.push(sp);
    }
    env_ptrs.reverse();

    let mut arg_ptrs = Vec::new();
    for arg in argv.iter().rev() {
        sp -= arg.len() as u64 + 1;
        sp &= !0x7;
        let mut buf = arg.to_vec();
        buf.push(0);
        aspace.copy_to_user(sp, &buf).map_err(|_| "arg write")?;
        arg_ptrs.push(sp);
    }
    arg_ptrs.reverse();

    sp &= !0xF;

    let total_items = 1 + arg_ptrs.len() + 1 + env_ptrs.len() + 1 + 2 * 10 + 2;
    if total_items % 2 != 0 { sp -= 8; }

    // Auxiliary vector
    push_u64(&mut sp, 0, aspace)?; push_u64(&mut sp, AT_NULL, aspace)?;
    push_u64(&mut sp, random_addr, aspace)?; push_u64(&mut sp, AT_RANDOM, aspace)?;
    push_u64(&mut sp, PAGE_SIZE as u64, aspace)?; push_u64(&mut sp, AT_PAGESZ, aspace)?;
    push_u64(&mut sp, elf_result.phdr_addr, aspace)?; push_u64(&mut sp, AT_PHDR, aspace)?;
    push_u64(&mut sp, elf_result.phent as u64, aspace)?; push_u64(&mut sp, AT_PHENT, aspace)?;
    push_u64(&mut sp, elf_result.phnum as u64, aspace)?; push_u64(&mut sp, AT_PHNUM, aspace)?;
    push_u64(&mut sp, elf_result.exe_entry, aspace)?; push_u64(&mut sp, AT_ENTRY, aspace)?;
    push_u64(&mut sp, elf_result.interp_base, aspace)?; push_u64(&mut sp, AT_BASE, aspace)?;
    push_u64(&mut sp, 0, aspace)?; push_u64(&mut sp, AT_UID, aspace)?;
    push_u64(&mut sp, 0, aspace)?; push_u64(&mut sp, AT_GID, aspace)?;

    push_u64(&mut sp, 0, aspace)?;
    for ptr in env_ptrs.iter().rev() { push_u64(&mut sp, *ptr, aspace)?; }

    push_u64(&mut sp, 0, aspace)?;
    for ptr in arg_ptrs.iter().rev() { push_u64(&mut sp, *ptr, aspace)?; }

    push_u64(&mut sp, argv.len() as u64, aspace)?;

    Ok(sp)
}

/// Load an ELF interpreter (ld-linux) at a given base address.
/// Returns the interpreter entry point.
fn load_interp(data: &[u8], aspace: &AddressSpace, base: u64) -> Result<u64, &'static str> {
    if data.len() < core::mem::size_of::<Header>() {
        return Err("Interpreter ELF too small");
    }
    let ehdr = unsafe { &*(data.as_ptr() as *const Header) };
    if ehdr.e_ident[0..4] != ELF_MAGIC { return Err("Interpreter: bad ELF magic"); }

    let phdrs = unsafe {
        core::slice::from_raw_parts(
            data.as_ptr().add(ehdr.e_phoff as usize) as *const ProgramHeader,
            ehdr.e_phnum as usize,
        )
    };

    let mut min_vaddr = u64::MAX;
    for phdr in phdrs {
        if phdr.p_type == PT_LOAD && phdr.p_vaddr < min_vaddr {
            min_vaddr = phdr.p_vaddr;
        }
    }
    let load_bias = base - (min_vaddr & !(PAGE_SIZE as u64 - 1));

    for phdr in phdrs {
        if phdr.p_type == PT_LOAD {
            let vaddr = phdr.p_vaddr + load_bias;
            let aligned_start = vaddr & !(PAGE_SIZE as u64 - 1);
            let aligned_end = (vaddr + phdr.p_memsz + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
            let map_size = (aligned_end - aligned_start) as usize;

            log::debug!("interp PT_LOAD: vaddr=0x{:x} size=0x{:x} flags=0x{:x}", aligned_start, map_size, phdr.p_flags);
            let flags = phdr_to_pageflags(phdr.p_flags);
            aspace.map_anon(aligned_start, map_size, flags)
                .map_err(|_| "Failed to map interp segment")?;

            if phdr.p_filesz > 0 {
                let file_data = &data[phdr.p_offset as usize..(phdr.p_offset + phdr.p_filesz) as usize];
                aspace.copy_to_user(vaddr, file_data)
                    .map_err(|_| "Failed to copy interp data")?;
            }
        }
    }

    Ok(ehdr.e_entry + load_bias)
}

fn phdr_to_pageflags(p_flags: u32) -> PageFlags {
    PageFlags {
        readable: p_flags & PF_R != 0,
        writable: p_flags & PF_W != 0,
        executable: p_flags & PF_X != 0,
        user: true,
        device: false,
    }
}
