use super::vfs::{self, Inode};
use alloc::string::ToString;
use alloc::sync::Arc;
use spin::Mutex;

pub fn init() {
    let root = vfs::root();
    let root_node = root.lock();
    if let Some(dev) = root_node.children.get("dev") {
        let mut dev_node = dev.lock();
        // /dev/null
        dev_node.children.insert("null".to_string(), Arc::new(Mutex::new(Inode::new_chardev(1, 3))));
        // /dev/zero
        dev_node.children.insert("zero".to_string(), Arc::new(Mutex::new(Inode::new_chardev(1, 5))));
        // /dev/urandom
        dev_node.children.insert("urandom".to_string(), Arc::new(Mutex::new(Inode::new_chardev(1, 9))));
        // /dev/tty
        dev_node.children.insert("tty".to_string(), Arc::new(Mutex::new(Inode::new_chardev(5, 0))));
        // /dev/console
        dev_node.children.insert("console".to_string(), Arc::new(Mutex::new(Inode::new_chardev(5, 1))));
        // /dev/fb0
        dev_node.children.insert("fb0".to_string(), Arc::new(Mutex::new(Inode::new_chardev(29, 0))));
        // /dev/dri directory
        let dri = Arc::new(Mutex::new(Inode::new_dir(0o755)));
        {
            let mut dri_node = dri.lock();
            dri_node.children.insert("card0".to_string(), Arc::new(Mutex::new(Inode::new_chardev(226, 0))));
            dri_node.children.insert("renderD128".to_string(), Arc::new(Mutex::new(Inode::new_chardev(226, 128))));
        }
        dev_node.children.insert("dri".to_string(), dri);
        // /dev/input directory
        let input = Arc::new(Mutex::new(Inode::new_dir(0o755)));
        {
            let mut input_node = input.lock();
            // event0 = keyboard (minor 64), event1 = mouse (minor 65)
            input_node.children.insert("event0".to_string(), Arc::new(Mutex::new(Inode::new_chardev(13, 64))));
            input_node.children.insert("event1".to_string(), Arc::new(Mutex::new(Inode::new_chardev(13, 65))));
            input_node.children.insert("mice".to_string(), Arc::new(Mutex::new(Inode::new_chardev(13, 63))));
        }
        dev_node.children.insert("input".to_string(), input);
        // /dev/ptmx — PTY master multiplexor (major 5, minor 2)
        dev_node.children.insert("ptmx".to_string(), Arc::new(Mutex::new(Inode::new_chardev(5, 2))));
        // /dev/pts directory (PTY slaves are opened via /dev/pts/N)
        dev_node.children.insert("pts".to_string(), Arc::new(Mutex::new(Inode::new_dir(0o755))));
    }
    log::debug!("devfs populated");
}
