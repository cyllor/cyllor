use super::vfs::{self, Inode, InodeType};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use spin::Mutex;

/// Dynamically read the current process's executable path from Process.name.
fn current_exe_path() -> String {
    let pid = crate::sched::process::current_pid();
    let table = crate::sched::process::PROCESS_TABLE.lock();
    if let Some(info) = table.get(&pid) {
        let name = &info.name;
        if name.starts_with('/') {
            return name.clone();
        }
        return alloc::format!("/usr/bin/{}", name);
    }
    "/usr/bin/init".to_string()
}

/// Build a dynamic /proc/self directory with per-process contents.
/// Called every time /proc/self (or /proc/<pid>) is resolved.
pub fn build_self_dir() -> Arc<Mutex<Inode>> {
    let self_dir = Arc::new(Mutex::new(Inode::new_dir(0o555)));
    let mut sd = self_dir.lock();

    let pid = crate::sched::process::current_pid();
    let proc_name = {
        let table = crate::sched::process::PROCESS_TABLE.lock();
        table.get(&pid).map(|i| i.name.clone()).unwrap_or_else(|| "init".to_string())
    };

    // /proc/self/exe -> symlink to the executable (dynamic per-process)
    let exe_path = current_exe_path();
    let mut exe = Inode::new_file(0o777);
    exe.itype = InodeType::Symlink;
    exe.data = exe_path.as_bytes().to_vec();
    exe.size = exe.data.len();
    sd.children.insert("exe".to_string(), Arc::new(Mutex::new(exe)));

    // /proc/self/cmdline (dynamic)
    let mut cmdline = Inode::new_file(0o444);
    cmdline.data = alloc::format!("{}\0", proc_name).into_bytes();
    cmdline.size = cmdline.data.len();
    sd.children.insert("cmdline".to_string(), Arc::new(Mutex::new(cmdline)));

    // /proc/self/maps — generate from per-process VMM if available,
    // otherwise return a minimal placeholder.
    let maps_str = {
        // Try to get the VMM from the Thread/scheduler infrastructure
        // For now, generate a sensible default based on the ELF loader layout
        alloc::format!(
            "00400000-00401000 r-xp 00000000 00:00 0          {}\n\
             7fffffffe000-7ffffffff000 rw-p 00000000 00:00 0          [stack]\n",
            exe_path
        )
    };
    let mut maps = Inode::new_file(0o444);
    maps.data = maps_str.into_bytes();
    maps.size = maps.data.len();
    sd.children.insert("maps".to_string(), Arc::new(Mutex::new(maps)));

    // /proc/self/status (dynamic)
    let status_str = alloc::format!(
        "Name:\t{}\nPid:\t{}\nTgid:\t{}\nPPid:\t0\nUid:\t0 0 0 0\nGid:\t0 0 0 0\nVmRSS:\t4096 kB\nThreads:\t1\n",
        proc_name, pid, pid,
    );
    let mut status = Inode::new_file(0o444);
    status.data = status_str.into_bytes();
    status.size = status.data.len();
    sd.children.insert("status".to_string(), Arc::new(Mutex::new(status)));

    // /proc/self/fd -> directory (empty stub)
    sd.children.insert("fd".to_string(), Arc::new(Mutex::new(Inode::new_dir(0o555))));

    // /proc/self/stat
    let stat_str = alloc::format!(
        "{} ({}) S 0 {} {} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 0 4096 1\n",
        pid, proc_name, pid, pid
    );
    let mut stat = Inode::new_file(0o444);
    stat.data = stat_str.into_bytes();
    stat.size = stat.data.len();
    sd.children.insert("stat".to_string(), Arc::new(Mutex::new(stat)));

    // /proc/self/statm
    let mut statm = Inode::new_file(0o444);
    statm.data = b"1024 256 128 64 0 960 0\n".to_vec();
    statm.size = statm.data.len();
    sd.children.insert("statm".to_string(), Arc::new(Mutex::new(statm)));

    // /proc/self/mounts (empty)
    sd.children.insert("mounts".to_string(), Arc::new(Mutex::new(Inode::new_file(0o444))));
    // /proc/self/environ (empty)
    sd.children.insert("environ".to_string(), Arc::new(Mutex::new(Inode::new_file(0o444))));

    // /proc/self/cgroup
    let mut cgroup = Inode::new_file(0o444);
    cgroup.data = b"0::/\n".to_vec();
    cgroup.size = cgroup.data.len();
    sd.children.insert("cgroup".to_string(), Arc::new(Mutex::new(cgroup)));

    drop(sd);
    self_dir
}

pub fn init() {
    let root = vfs::root();
    let root_node = root.lock();
    if let Some(proc) = root_node.children.get("proc") {
        let mut proc_node = proc.lock();

        // /proc/self placeholder (rebuilt dynamically on each access via build_self_dir)
        let self_dir = build_self_dir();
        proc_node.children.insert("self".to_string(), self_dir.clone());
        proc_node.children.insert("1".to_string(), self_dir);

        // /proc/meminfo
        let mut meminfo = Inode::new_file(0o444);
        meminfo.data = b"MemTotal:      524288 kB\nMemFree:       480000 kB\nMemAvailable:  490000 kB\n".to_vec();
        meminfo.size = meminfo.data.len();
        proc_node.children.insert("meminfo".to_string(), Arc::new(Mutex::new(meminfo)));

        // /proc/cpuinfo
        let mut cpuinfo = Inode::new_file(0o444);
        cpuinfo.data = b"processor\t: 0\nmodel name\t: Cyllor Virtual CPU\n".to_vec();
        cpuinfo.size = cpuinfo.data.len();
        proc_node.children.insert("cpuinfo".to_string(), Arc::new(Mutex::new(cpuinfo)));

        // /proc/version
        let mut version = Inode::new_file(0o444);
        version.data = b"Cyllor OS 0.1.0 (rustc)\n".to_vec();
        version.size = version.data.len();
        proc_node.children.insert("version".to_string(), Arc::new(Mutex::new(version)));

        // /proc/sys/kernel/pid_max
        let proc_sys_dir = Arc::new(Mutex::new(Inode::new_dir(0o555)));
        {
            let mut proc_sys_node = proc_sys_dir.lock();
            let kernel_dir = Arc::new(Mutex::new(Inode::new_dir(0o555)));
            {
                let mut kernel_node = kernel_dir.lock();
                let mut pid_max = Inode::new_file(0o444);
                pid_max.data = b"32768\n".to_vec();
                pid_max.size = pid_max.data.len();
                kernel_node.children.insert("pid_max".to_string(), Arc::new(Mutex::new(pid_max)));
            }
            proc_sys_node.children.insert("kernel".to_string(), kernel_dir);
        }
        proc_node.children.insert("sys".to_string(), proc_sys_dir);

        // /proc/filesystems
        let mut filesystems = Inode::new_file(0o444);
        filesystems.data = b"nodev\ttmpfs\nnodev\tproc\nnodev\tsysfs\n\text4\n".to_vec();
        filesystems.size = filesystems.data.len();
        proc_node.children.insert("filesystems".to_string(), Arc::new(Mutex::new(filesystems)));
    }
    log::debug!("procfs populated");
}
