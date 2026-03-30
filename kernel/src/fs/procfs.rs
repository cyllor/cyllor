use super::vfs::{self, Inode};
use alloc::string::ToString;
use alloc::sync::Arc;
use spin::Mutex;

pub fn init() {
    let root = vfs::root();
    let root_node = root.lock();
    if let Some(proc) = root_node.children.get("proc") {
        let mut proc_node = proc.lock();
        // /proc/self -> symlink-like (we just create a directory)
        let self_dir = Arc::new(Mutex::new(Inode::new_dir(0o555)));
        proc_node.children.insert("self".to_string(), self_dir);

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
    }
    log::debug!("procfs populated");
}
