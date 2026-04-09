pub mod cpu;
pub mod elf;
pub mod process;
pub mod scheduler;
pub mod wait;

pub use process::{Context, Pid, Process, Thread, ThreadState};
pub use scheduler::SCHEDULER;

use crate::syscall::{SyscallResult, ECHILD};
use crate::arch::CpuContext;
use crate::mm::vmm::Vmm;
use crate::arch::PageAttr;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::Mutex;

// child_pid -> (parent_pid, exit_code)
static EXITED_CHILDREN: Mutex<BTreeMap<Pid, (Pid, i32)>> = Mutex::new(BTreeMap::new());

/// Called on every timer interrupt.
/// Advances the sleep clock, then schedules on the current CPU.
pub fn timer_tick() {
    wait::tick();
    scheduler::schedule();
}

/// Initialize the scheduler with one idle thread per CPU.
pub fn init(num_cpus: usize) {
    scheduler::init(num_cpus);
}

/// Spawn a kernel thread, distributing it to the least-loaded CPU.
pub fn spawn_kernel_thread(name: &str, entry: fn()) -> Pid {
    scheduler::spawn_kernel_thread(name, entry)
}

/// Load and start a user process from the VFS, distributing it to the
/// least-loaded CPU and sending a resched IPI if the target is remote.
pub fn spawn_user_process(
    path: &str,
    argv: &[&[u8]],
    envp: &[&[u8]],
) -> Result<Pid, &'static str> {
    let argv_vec: alloc::vec::Vec<alloc::vec::Vec<u8>> = argv.iter().map(|a| a.to_vec()).collect();
    let (exec_path, exec_argv, data) = resolve_exec_image(path, argv_vec).map_err(|_| "file not found")?;

    let aspace = crate::arch::create_user_page_table()
        .ok_or("failed to allocate address space")?;

    let result = elf::load_elf(&data, aspace.as_ref())?;
    let exec_argv_refs: alloc::vec::Vec<&[u8]> = exec_argv.iter().map(|a| a.as_slice()).collect();
    let sp = elf::setup_user_stack(aspace.as_ref(), result.stack_top, &exec_argv_refs, envp, &result)?;

    let pid = process::alloc_pid();
    let ppid = process::current_pid();
    let (uid, gid, euid, egid, suid, sgid) = {
        let table = process::PROCESS_TABLE.lock();
        if let Some(parent) = table.get(&ppid) {
            (parent.uid, parent.gid, parent.euid, parent.egid, parent.suid, parent.sgid)
        } else {
            (0, 0, 0, 0, 0, 0)
        }
    };
    let thread = Thread::new_user(&exec_path, pid, result.entry, sp, aspace);
    let env_blob = pack_envp(envp);

    {
        let mut table = process::PROCESS_TABLE.lock();
        table.insert(pid, process::Process {
            pid,
            tgid: pid,
            pgid: pid,
            sid: pid,
            ppid,
            uid,
            gid,
            euid,
            egid,
            suid,
            sgid,
            no_new_privs: false,
            name: exec_path,
            environ: env_blob,
            threads: alloc::vec![pid],
            brk_base: result.brk_start as usize,
            brk_current: result.brk_start as usize,
            mmap_next: crate::arch::USER_MMAP_BASE,
            vmm: Arc::new(Mutex::new(Vmm::new())),
        });
    }

    let target_cpu;
    {
        let mut sched = scheduler::SCHEDULER.lock();
        if sched.run_queues.is_empty() {
            return Err("scheduler not initialized");
        }
        target_cpu = scheduler::least_loaded_cpu(&sched);
        sched.run_queues[target_cpu].push_back(thread);
    }

    if target_cpu != cpu::current_cpu_id() {
        scheduler::send_resched_ipi(target_cpu);
    }

    log::info!(
        "Spawned user process '{}' (PID {pid}) entry=0x{:x} on CPU {target_cpu}",
        path,
        result.entry
    );
    Ok(pid)
}

pub fn note_process_exit(pid: Pid, code: i32) {
    crate::syscall::cleanup_thread_state(pid);
    crate::mm::shm::cleanup_process_attachments(pid);
    let (parent, tgid) = {
        let mut table = process::PROCESS_TABLE.lock();
        let info = table.get(&pid).map(|p| (p.ppid, p.tgid)).unwrap_or((0, pid));
        // Remove from thread-group leader list if present.
        if let Some(leader) = table.get_mut(&info.1) {
            leader.threads.retain(|&tid| tid != pid);
        }
        table.remove(&pid);
        info
    };
    // Only thread-group leaders are waitable children.
    if parent != 0 && tgid == pid {
        EXITED_CHILDREN.lock().insert(pid, (parent, code));
    }
}

pub fn note_thread_group_exit(tgid: Pid, code: i32) {
    let members: alloc::vec::Vec<Pid> = {
        let table = process::PROCESS_TABLE.lock();
        table
            .values()
            .filter(|p| p.tgid == tgid)
            .map(|p| p.pid)
            .collect()
    };
    for pid in members {
        note_process_exit(pid, code);
    }
}

fn read_user_cstr(ptr: u64, max_len: usize) -> Result<alloc::string::String, i32> {
    if ptr == 0 {
        return Err(crate::syscall::EINVAL);
    }
    let mut buf = alloc::vec![0u8; max_len];
    let mut len = 0usize;
    while len < max_len - 1 {
        let mut one = [0u8; 1];
        crate::syscall::fs::copy_from_user(ptr + len as u64, &mut one).map_err(|_| crate::syscall::EFAULT)?;
        if one[0] == 0 {
            break;
        }
        buf[len] = one[0];
        len += 1;
    }
    core::str::from_utf8(&buf[..len])
        .map(|s| alloc::string::String::from(s))
        .map_err(|_| crate::syscall::EINVAL)
}

fn read_user_ptr_array(ptr: u64, max_items: usize, max_str_len: usize) -> Result<alloc::vec::Vec<alloc::vec::Vec<u8>>, i32> {
    let mut out = alloc::vec::Vec::new();
    if ptr == 0 {
        return Ok(out);
    }
    for i in 0..max_items {
        let mut raw = [0u8; 8];
        crate::syscall::fs::copy_from_user(ptr + (i as u64) * 8, &mut raw).map_err(|_| crate::syscall::EFAULT)?;
        let p = u64::from_le_bytes(raw);
        if p == 0 {
            break;
        }
        let s = read_user_cstr(p, max_str_len)?;
        out.push(s.into_bytes());
    }
    Ok(out)
}

fn pack_envp(envp: &[&[u8]]) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::new();
    for e in envp {
        out.extend_from_slice(e);
        out.push(0);
    }
    out
}

fn load_program_data(path: &str) -> Result<alloc::vec::Vec<u8>, i32> {
    if let Ok(node) = crate::fs::vfs::resolve_path(path) {
        let data = node.lock().data.clone();
        if !data.is_empty() {
            return Ok(data);
        }
    }
    crate::fs::ext4::read_file(path).map_err(|_| crate::syscall::ENOENT)
}

fn parse_shebang(data: &[u8]) -> Option<(alloc::string::String, Option<alloc::string::String>)> {
    if data.len() < 2 || data[0] != b'#' || data[1] != b'!' {
        return None;
    }
    let end = data.iter().position(|b| *b == b'\n').unwrap_or(data.len());
    let line = core::str::from_utf8(&data[2..end]).ok()?.trim();
    if line.is_empty() {
        return None;
    }
    let mut parts = line.split_whitespace();
    let interp = parts.next()?;
    let arg = parts.next().map(alloc::string::String::from);
    Some((alloc::string::String::from(interp), arg))
}

fn resolve_exec_image(
    requested_path: &str,
    mut argv: alloc::vec::Vec<alloc::vec::Vec<u8>>,
) -> Result<(alloc::string::String, alloc::vec::Vec<alloc::vec::Vec<u8>>, alloc::vec::Vec<u8>), i32> {
    let data = load_program_data(requested_path)?;
    if let Some((interp, optarg)) = parse_shebang(&data) {
        let interp_data = load_program_data(&interp)?;
        let mut script_argv = alloc::vec::Vec::new();
        script_argv.push(interp.as_bytes().to_vec());
        if let Some(a) = optarg {
            script_argv.push(a.into_bytes());
        }
        script_argv.push(requested_path.as_bytes().to_vec());
        if argv.len() > 1 {
            script_argv.extend(argv.drain(1..));
        }
        return Ok((interp, script_argv, interp_data));
    }
    Ok((alloc::string::String::from(requested_path), argv, data))
}

fn prot_to_page_attr(prot: u32) -> PageAttr {
    let readable = (prot & crate::mm::vmm::PROT_READ) != 0;
    let writable = (prot & crate::mm::vmm::PROT_WRITE) != 0;
    let executable = (prot & crate::mm::vmm::PROT_EXEC) != 0;
    PageAttr {
        readable: readable || writable || executable,
        writable,
        executable,
        user: true,
        device: false,
    }
}

fn clone_user_address_space(
    parent_root: u64,
    child_root: u64,
    parent_vmm: &Vmm,
) -> Result<(), i32> {
    let hhdm = crate::arch::hhdm_offset();
    let mut mapped_pages: alloc::vec::Vec<u64> = alloc::vec::Vec::new();

    for vma in parent_vmm.snapshot() {
        let mut va = vma.start;
        while va < vma.end {
            if let Some(parent_phys) = crate::arch::translate_user_va(parent_root, va) {
                let child_phys = match crate::mm::pmm::alloc_page() {
                    Some(p) => p,
                    None => {
                        for mapped_va in mapped_pages.drain(..) {
                            if let Some(phys) = crate::arch::unmap_user_page(child_root, mapped_va) {
                                crate::mm::pmm::free_page(phys as usize);
                            }
                        }
                        return Err(crate::syscall::ENOMEM);
                    }
                };
                let child_phys_u64 = child_phys as u64;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        (parent_phys + hhdm) as *const u8,
                        (child_phys_u64 + hhdm) as *mut u8,
                        4096,
                    );
                }
                let flags = prot_to_page_attr(vma.prot);
                crate::arch::map_user_page(child_root, va, child_phys_u64, flags);
                mapped_pages.push(va);
            }
            va += 4096;
        }
    }

    Ok(())
}

/// clone syscall 鈥?create a runnable child user context.
pub fn do_clone(frame: &mut impl CpuContext, flags: u64, stack: u64, ptid: u64, tls: u64, ctid: u64) -> SyscallResult {
    const CLONE_VM: u64 = 0x0000_0100;
    const CLONE_SIGHAND: u64 = 0x0000_0800;
    const CLONE_SETTLS: u64 = 0x0008_0000;
    const CLONE_PARENT: u64 = 0x0000_8000;
    const CLONE_PIDFD: u64 = 0x0000_1000;
    const CLONE_THREAD: u64 = 0x0001_0000;
    const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
    const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
    const CLONE_CHILD_SETTID: u64 = 0x0100_0000;

    let exit_signal = flags & 0xff;

    // Linux requires CLONE_THREAD => CLONE_SIGHAND => CLONE_VM.
    if (flags & CLONE_THREAD) != 0 {
        if (flags & CLONE_SIGHAND) == 0 || (flags & CLONE_VM) == 0 {
            return Err(crate::syscall::EINVAL);
        }
        // Thread exits should not deliver a signal to parent.
        if exit_signal != 0 {
            return Err(crate::syscall::EINVAL);
        }
    }
    if (flags & CLONE_SIGHAND) != 0 && (flags & CLONE_VM) == 0 {
        return Err(crate::syscall::EINVAL);
    }
    if (flags & CLONE_PIDFD) != 0 && (flags & CLONE_PARENT_SETTID) != 0 {
        return Err(crate::syscall::EINVAL);
    }
    if (flags & CLONE_PIDFD) != 0 && ptid == 0 {
        return Err(crate::syscall::EINVAL);
    }
    let share_vm = (flags & CLONE_VM) != 0;
    let parent_pid = process::current_pid();
    let child = process::alloc_pid();
    let child_sp = if stack != 0 { stack } else { frame.sp() };
    let child_pc = frame.pc();

    let (
        parent_name,
        parent_tgid,
        parent_pgid,
        parent_sid,
        parent_ppid,
        parent_uid,
        parent_gid,
        parent_euid,
        parent_egid,
        parent_suid,
        parent_sgid,
        parent_no_new_privs,
        parent_environ,
        parent_brk_base,
        parent_brk_current,
        parent_mmap_next,
        parent_vmm,
    ) = {
        let table = process::PROCESS_TABLE.lock();
        if let Some(p) = table.get(&parent_pid) {
            (
                p.name.clone(),
                p.tgid,
                p.pgid,
                p.sid,
                p.ppid,
                p.uid,
                p.gid,
                p.euid,
                p.egid,
                p.suid,
                p.sgid,
                p.no_new_privs,
                p.environ.clone(),
                p.brk_base,
                p.brk_current,
                p.mmap_next,
                Arc::clone(&p.vmm),
            )
        } else {
            (
                alloc::string::String::from("cloned"),
                parent_pid,
                parent_pid,
                parent_pid,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                false,
                alloc::vec::Vec::new(),
                crate::arch::USER_BRK_BASE,
                crate::arch::USER_BRK_BASE,
                crate::arch::USER_MMAP_BASE,
                Arc::new(Mutex::new(Vmm::new())),
            )
        }
    };

    let (page_root, inherited_tls) = {
        let cpu_id = cpu::current_cpu_id();
        let sched = scheduler::SCHEDULER.lock();
        let curr = sched.current[cpu_id].as_ref().ok_or(crate::syscall::ESRCH)?;
        (curr.context.page_table_root, curr.context.user_tls)
    };
    let child_tls = if (flags & CLONE_SETTLS) != 0 { tls } else { inherited_tls };

    let parent_vmm_clone = parent_vmm.lock().clone();

    let mut thread = if share_vm {
        process::Thread::new_user_cloned(
            &parent_name,
            child,
            child_pc,
            child_sp,
            page_root,
            child_tls,
        )
    } else {
        let child_aspace = crate::arch::create_user_page_table().ok_or(crate::syscall::ENOMEM)?;
        let child_root = child_aspace.root_phys();
        clone_user_address_space(page_root, child_root, &parent_vmm_clone)?;
        let mut t = process::Thread::new_user(&parent_name, child, child_pc, child_sp, child_aspace);
        t.context.user_tls = child_tls;
        t
    };
    thread.context.user_tls = child_tls;

    let target_cpu;
    {
        let mut sched = scheduler::SCHEDULER.lock();
        if sched.run_queues.is_empty() {
            return Err(crate::syscall::ESRCH);
        }
        target_cpu = scheduler::least_loaded_cpu(&sched);
        sched.run_queues[target_cpu].push_back(thread);
    }
    if target_cpu != cpu::current_cpu_id() {
        scheduler::send_resched_ipi(target_cpu);
    }

    {
        let mut table = process::PROCESS_TABLE.lock();
        let tgid = if (flags & CLONE_THREAD) != 0 { parent_tgid } else { child };
        let ppid = if (flags & CLONE_THREAD) != 0 {
            parent_ppid
        } else if (flags & CLONE_PARENT) != 0 {
            parent_ppid
        } else {
            parent_pid
        };
        table.insert(child, process::Process {
            pid: child,
            tgid,
            pgid: parent_pgid,
            sid: parent_sid,
            ppid,
            uid: parent_uid,
            gid: parent_gid,
            euid: parent_euid,
            egid: parent_egid,
            suid: parent_suid,
            sgid: parent_sgid,
            no_new_privs: parent_no_new_privs,
            name: parent_name,
            environ: parent_environ,
            threads: alloc::vec![child],
            brk_base: parent_brk_base,
            brk_current: parent_brk_current,
            mmap_next: parent_mmap_next,
            vmm: if share_vm {
                parent_vmm
            } else {
                Arc::new(Mutex::new(parent_vmm_clone.clone()))
            },
        });
        if (flags & CLONE_THREAD) != 0 {
            if let Some(leader) = table.get_mut(&parent_tgid) {
                leader.threads.push(child);
            }
        }
    }

    if (flags & CLONE_PARENT_SETTID) != 0 && ptid != 0 {
        let _ = crate::syscall::fs::copy_to_user(ptid, &(child as u64).to_le_bytes());
    }
    if (flags & CLONE_PIDFD) != 0 {
        let pidfd = crate::fs::alloc_pidfd(child)? as i32;
        let _ = crate::syscall::fs::copy_to_user(ptid, &pidfd.to_le_bytes());
    }
    if (flags & CLONE_CHILD_SETTID) != 0 && ctid != 0 {
        let _ = crate::syscall::fs::copy_to_user(ctid, &(child as u64).to_le_bytes());
    }
    if (flags & CLONE_CHILD_CLEARTID) != 0 && ctid != 0 {
        crate::syscall::register_clear_child_tid(child, ctid);
    }

    Ok(child as usize)
}

/// execve syscall 鈥?Phase 6.
pub fn do_execve(frame: &mut impl CpuContext, pathname: u64, argv: u64, envp: u64) -> SyscallResult {
    let path = read_user_cstr(pathname, 512)?;
    let argv_vec = read_user_ptr_array(argv, 64, 256)?;
    let envp_vec = read_user_ptr_array(envp, 128, 512)?;
    let (exec_path, exec_argv, data) = resolve_exec_image(&path, argv_vec)?;
    let argv_refs: alloc::vec::Vec<&[u8]> = exec_argv.iter().map(|v| v.as_slice()).collect();
    let envp_refs: alloc::vec::Vec<&[u8]> = envp_vec.iter().map(|v| v.as_slice()).collect();
    let env_blob = pack_envp(&envp_refs);

    let aspace = crate::arch::create_user_page_table().ok_or(crate::syscall::ENOMEM)?;
    let new_root = aspace.root_phys();
    let result = elf::load_elf(&data, aspace.as_ref()).map_err(|_| crate::syscall::ENOENT)?;
    let sp = elf::setup_user_stack(aspace.as_ref(), result.stack_top, &argv_refs, &envp_refs, &result)
        .map_err(|_| crate::syscall::ENOMEM)?;

    let cpu_id = cpu::current_cpu_id();
    let pid = process::current_pid();
    {
        let mut sched = scheduler::SCHEDULER.lock();
        let curr = sched.current[cpu_id].as_mut().ok_or(crate::syscall::ESRCH)?;
        curr.name = exec_path.clone();
        curr.is_user = true;
        curr.first_run = false;
        curr.context.user_pc = result.entry;
        curr.context.user_sp = sp;
        curr.context.page_table_root = new_root;
        curr.context.user_tls = 0;
        curr.address_space = Some(aspace);
    }
    {
        let mut table = process::PROCESS_TABLE.lock();
        if let Some(proc) = table.get_mut(&pid) {
            proc.name = exec_path;
            proc.environ = env_blob;
            proc.brk_base = result.brk_start as usize;
            proc.brk_current = result.brk_start as usize;
            proc.mmap_next = crate::arch::USER_MMAP_BASE;
            proc.vmm = Arc::new(Mutex::new(Vmm::new()));
        }
    }

    // Apply close-on-exec after new image is successfully prepared.
    crate::fs::fdtable::close_cloexec_fds();

    crate::arch::activate_user_page_table(new_root);
    frame.set_pc(result.entry);
    frame.set_sp(sp);
    Ok(0)
}

/// wait4 syscall 鈥?compatibility implementation with basic blocking semantics.
pub fn do_wait4_with_status(pid: i32, options: u32) -> Result<(usize, i32), i32> {
    const WNOHANG: u32 = 1;
    const WNOWAIT: u32 = 0x0100_0000;
    let parent = process::current_pid();
    loop {
        let target = {
            let children = EXITED_CHILDREN.lock();
            if pid > 0 {
                let p = pid as u64;
                if children.get(&p).map(|(ppid, _)| *ppid == parent).unwrap_or(false) {
                    Some(p)
                } else {
                    None
                }
            } else {
                children
                    .iter()
                    .find(|(_, (ppid, _))| *ppid == parent)
                    .map(|(&p, _)| p)
            }
        };

        if let Some(target) = target {
            let status = if (options & WNOWAIT) != 0 {
                let children = EXITED_CHILDREN.lock();
                children.get(&target).map(|(_, st)| *st).unwrap_or(0)
            } else {
                let mut children = EXITED_CHILDREN.lock();
                children.remove(&target).map(|(_, st)| st).unwrap_or(0)
            };
            return Ok((target as usize, status));
        }

        let has_live = {
            let table = process::PROCESS_TABLE.lock();
            if pid > 0 {
                table
                    .values()
                    .any(|proc| proc.ppid == parent && proc.pid == proc.tgid && proc.pid == pid as u64)
            } else {
                table
                    .values()
                    .any(|proc| proc.ppid == parent && proc.pid == proc.tgid)
            }
        };

        if has_live {
            if (options & WNOHANG) != 0 {
                return Ok((0, 0));
            }
            wait::sleep_ticks(1);
            continue;
        }

        let has_live = {
            let children = EXITED_CHILDREN.lock();
            if pid > 0 {
                children.get(&(pid as u64)).map(|(ppid, _)| *ppid == parent).unwrap_or(false)
            } else {
                children.values().any(|(ppid, _)| *ppid == parent)
            }
        };
        if has_live {
            continue;
        }
        return Err(ECHILD);
    }
}

/// wait4 syscall — compatibility implementation with basic blocking semantics.
pub fn do_wait4(pid: i32, wstatus: u64, options: u32, _rusage: u64) -> SyscallResult {
    let (reaped, status) = do_wait4_with_status(pid, options)?;
    if reaped != 0 && wstatus != 0 {
        let raw = ((status as u32) << 8).to_le_bytes();
        let _ = crate::syscall::fs::copy_to_user(wstatus, &raw);
    }
    Ok(reaped)
}
