use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::syscall::{SyscallResult, ENOENT, EEXIST, ENOTDIR, EISDIR, EINVAL, ENOSYS};

/// Inode types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InodeType {
    File,
    Directory,
    CharDevice,
    BlockDevice,
    Pipe,
    Socket,
    Symlink,
}

/// An inode in the VFS
pub struct Inode {
    pub itype: InodeType,
    pub size: usize,
    pub data: Vec<u8>,
    pub children: BTreeMap<String, Arc<Mutex<Inode>>>,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub dev_major: u32,
    pub dev_minor: u32,
}

impl Inode {
    pub fn new_dir(mode: u32) -> Self {
        Self {
            itype: InodeType::Directory,
            size: 0,
            data: Vec::new(),
            children: BTreeMap::new(),
            mode,
            uid: 0, gid: 0,
            dev_major: 0, dev_minor: 0,
        }
    }

    pub fn new_file(mode: u32) -> Self {
        Self {
            itype: InodeType::File,
            size: 0,
            data: Vec::new(),
            children: BTreeMap::new(),
            mode,
            uid: 0, gid: 0,
            dev_major: 0, dev_minor: 0,
        }
    }

    pub fn new_chardev(major: u32, minor: u32) -> Self {
        Self {
            itype: InodeType::CharDevice,
            size: 0,
            data: Vec::new(),
            children: BTreeMap::new(),
            mode: 0o666,
            uid: 0, gid: 0,
            dev_major: major, dev_minor: minor,
        }
    }
}

/// File object - represents an open file
pub struct FileObject {
    pub inode: Option<Arc<Mutex<Inode>>>,
    pub offset: usize,
    pub flags: u32,
    pub ftype: FileType,
    // For special files
    pub special_data: Option<SpecialData>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileType {
    Regular,
    Directory,
    CharDevice,
    Pipe,
    Epoll,
    EventFd,
    MemFd,
    Socket,
}

pub enum SpecialData {
    EventFdVal(u64),
    PipeBuffer(Arc<Mutex<Vec<u8>>>),
    MemFdData(Vec<u8>),
}

impl FileObject {
    pub fn new(inode: Arc<Mutex<Inode>>, flags: u32) -> Self {
        let itype = inode.lock().itype;
        let ftype = match itype {
            InodeType::File => FileType::Regular,
            InodeType::Directory => FileType::Directory,
            InodeType::CharDevice => FileType::CharDevice,
            _ => FileType::Regular,
        };
        Self { inode: Some(inode), offset: 0, flags, ftype, special_data: None }
    }

    pub fn new_epoll() -> Self {
        Self { inode: None, offset: 0, flags: 0, ftype: FileType::Epoll, special_data: None }
    }

    pub fn new_eventfd(initval: u32) -> Self {
        Self {
            inode: None, offset: 0, flags: 0, ftype: FileType::EventFd,
            special_data: Some(SpecialData::EventFdVal(initval as u64)),
        }
    }

    pub fn new_memfd() -> Self {
        Self {
            inode: None, offset: 0, flags: 0, ftype: FileType::MemFd,
            special_data: Some(SpecialData::MemFdData(Vec::new())),
        }
    }

    pub fn new_pipe(buf: Arc<Mutex<Vec<u8>>>, is_read: bool) -> Self {
        Self {
            inode: None, offset: 0,
            flags: if is_read { 0 } else { 1 },
            ftype: FileType::Pipe,
            special_data: Some(SpecialData::PipeBuffer(buf)),
        }
    }

    pub fn read(&mut self, buf: u64, count: usize) -> SyscallResult {
        match self.ftype {
            FileType::Regular | FileType::MemFd => {
                let data = if let Some(ref inode) = self.inode {
                    let node = inode.lock();
                    let avail = node.data.len().saturating_sub(self.offset);
                    let to_read = count.min(avail);
                    if to_read > 0 {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                node.data[self.offset..].as_ptr(),
                                buf as *mut u8,
                                to_read,
                            );
                        }
                    }
                    self.offset += to_read;
                    to_read
                } else if let Some(SpecialData::MemFdData(ref data)) = self.special_data {
                    let avail = data.len().saturating_sub(self.offset);
                    let to_read = count.min(avail);
                    if to_read > 0 {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                data[self.offset..].as_ptr(), buf as *mut u8, to_read,
                            );
                        }
                    }
                    self.offset += to_read;
                    to_read
                } else {
                    0
                };
                Ok(data)
            }
            FileType::CharDevice => {
                // Character device read - stub
                Ok(0)
            }
            FileType::Pipe => {
                if let Some(SpecialData::PipeBuffer(ref pipe_buf)) = self.special_data {
                    let mut pb = pipe_buf.lock();
                    let to_read = count.min(pb.len());
                    if to_read > 0 {
                        unsafe {
                            core::ptr::copy_nonoverlapping(pb.as_ptr(), buf as *mut u8, to_read);
                        }
                        pb.drain(..to_read);
                    }
                    Ok(to_read)
                } else {
                    Ok(0)
                }
            }
            FileType::EventFd => {
                if count < 8 { return Err(EINVAL); }
                if let Some(SpecialData::EventFdVal(ref mut val)) = self.special_data {
                    unsafe { *(buf as *mut u64) = *val; }
                    *val = 0;
                    Ok(8)
                } else {
                    Ok(0)
                }
            }
            _ => Ok(0),
        }
    }

    pub fn write(&mut self, buf: u64, count: usize) -> SyscallResult {
        match self.ftype {
            FileType::Regular | FileType::MemFd => {
                if let Some(ref inode) = self.inode {
                    let mut node = inode.lock();
                    let end = self.offset + count;
                    if end > node.data.len() {
                        node.data.resize(end, 0);
                    }
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            buf as *const u8, node.data[self.offset..].as_mut_ptr(), count,
                        );
                    }
                    node.size = node.data.len();
                    self.offset = end;
                } else if let Some(SpecialData::MemFdData(ref mut data)) = self.special_data {
                    let end = self.offset + count;
                    if end > data.len() {
                        data.resize(end, 0);
                    }
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            buf as *const u8, data[self.offset..].as_mut_ptr(), count,
                        );
                    }
                    self.offset = end;
                }
                Ok(count)
            }
            FileType::CharDevice => {
                // stdout/stderr handled in syscall layer
                Ok(count)
            }
            FileType::Pipe => {
                if let Some(SpecialData::PipeBuffer(ref pipe_buf)) = self.special_data {
                    let mut pb = pipe_buf.lock();
                    let slice = unsafe { core::slice::from_raw_parts(buf as *const u8, count) };
                    pb.extend_from_slice(slice);
                    Ok(count)
                } else {
                    Ok(0)
                }
            }
            FileType::EventFd => {
                if count < 8 { return Err(EINVAL); }
                let val = unsafe { *(buf as *const u64) };
                if let Some(SpecialData::EventFdVal(ref mut v)) = self.special_data {
                    *v = v.wrapping_add(val);
                }
                Ok(8)
            }
            _ => Ok(count),
        }
    }

    pub fn lseek(&mut self, offset: i64, whence: u32) -> SyscallResult {
        let size = if let Some(ref inode) = self.inode {
            inode.lock().data.len()
        } else {
            0
        };

        let new_offset = match whence {
            0 => offset as usize,                          // SEEK_SET
            1 => (self.offset as i64 + offset) as usize,   // SEEK_CUR
            2 => (size as i64 + offset) as usize,           // SEEK_END
            _ => return Err(EINVAL),
        };

        self.offset = new_offset;
        Ok(new_offset)
    }

    pub fn stat(&self, statbuf: u64) -> SyscallResult {
        // struct stat for AArch64 Linux
        // Fill with reasonable defaults
        let stat_ptr = statbuf as *mut u8;
        unsafe {
            core::ptr::write_bytes(stat_ptr, 0, 128); // zero out
            // st_mode at offset 16
            let mode: u32 = if let Some(ref inode) = self.inode {
                let node = inode.lock();
                let type_bits = match node.itype {
                    InodeType::File => 0o100000,
                    InodeType::Directory => 0o040000,
                    InodeType::CharDevice => 0o020000,
                    _ => 0o100000,
                };
                type_bits | node.mode
            } else {
                match self.ftype {
                    FileType::Pipe => 0o010000 | 0o600,
                    _ => 0o100000 | 0o644,
                }
            };
            *((statbuf + 16) as *mut u32) = mode;

            // st_size at offset 48
            let size = if let Some(ref inode) = self.inode {
                inode.lock().size as u64
            } else { 0 };
            *((statbuf + 48) as *mut u64) = size;

            // st_blksize at offset 56
            *((statbuf + 56) as *mut u64) = 4096;
        }
        Ok(0)
    }

    pub fn ioctl(&mut self, request: u64, arg: u64) -> SyscallResult {
        // Check if this is a DRM device
        if let Some(ref inode) = self.inode {
            let node = inode.lock();
            if node.dev_major == 226 {
                // DRM device
                return crate::drivers::drm::handle_ioctl(request, arg);
            }
        }

        // Common ioctls
        match request {
            0x5401 => { // TCGETS - terminal attrs
                if arg != 0 {
                    unsafe { core::ptr::write_bytes(arg as *mut u8, 0, 60); }
                }
                Ok(0)
            }
            0x5413 => { // TIOCGWINSZ
                if arg != 0 {
                    unsafe {
                        *((arg) as *mut u16) = 25;   // rows
                        *((arg + 2) as *mut u16) = 80; // cols
                        *((arg + 4) as *mut u16) = 0;
                        *((arg + 6) as *mut u16) = 0;
                    }
                }
                Ok(0)
            }
            _ => Ok(0), // Accept unknown ioctls silently
        }
    }
}

// Global root filesystem
static ROOT: Mutex<Option<Arc<Mutex<Inode>>>> = Mutex::new(None);
static CWD: Mutex<String> = Mutex::new(String::new());

pub fn init() {
    let mut root = Inode::new_dir(0o755);
    // Create standard directories
    for dir in &["dev", "proc", "tmp", "etc", "var", "home", "usr", "bin", "sbin", "lib", "sys", "run"] {
        root.children.insert(dir.to_string(), Arc::new(Mutex::new(Inode::new_dir(0o755))));
    }

    // Create /usr/bin, /usr/lib, etc.
    if let Some(usr) = root.children.get("usr") {
        let mut usr_node = usr.lock();
        usr_node.children.insert("bin".to_string(), Arc::new(Mutex::new(Inode::new_dir(0o755))));
        usr_node.children.insert("lib".to_string(), Arc::new(Mutex::new(Inode::new_dir(0o755))));
        usr_node.children.insert("share".to_string(), Arc::new(Mutex::new(Inode::new_dir(0o755))));
    }

    *ROOT.lock() = Some(Arc::new(Mutex::new(root)));
    *CWD.lock() = "/".to_string();
}

pub fn root() -> Arc<Mutex<Inode>> {
    ROOT.lock().as_ref().unwrap().clone()
}

pub fn current_dir() -> String {
    CWD.lock().clone()
}

pub fn set_current_dir(path: &str) -> SyscallResult {
    // Verify path exists and is a directory
    let node = resolve_path(path)?;
    let n = node.lock();
    if n.itype != InodeType::Directory {
        return Err(ENOTDIR);
    }
    drop(n);
    *CWD.lock() = path.to_string();
    Ok(0)
}

pub fn resolve_path(path: &str) -> Result<Arc<Mutex<Inode>>, i32> {
    let root = root();

    if path == "/" || path.is_empty() {
        return Ok(root);
    }

    let path = path.trim_start_matches('/');
    let mut current = root;

    for component in path.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            // For simplicity, stay at root
            continue;
        }

        let next = {
            let node = current.lock();
            if node.itype != InodeType::Directory {
                return Err(ENOTDIR);
            }
            node.children.get(component).cloned()
        };

        match next {
            Some(child) => current = child,
            None => return Err(ENOENT),
        }
    }

    Ok(current)
}

pub fn open_node(inode: Arc<Mutex<Inode>>, flags: u32) -> Result<Arc<Mutex<FileObject>>, i32> {
    Ok(Arc::new(Mutex::new(FileObject::new(inode, flags))))
}

pub fn stat_node(inode: &Arc<Mutex<Inode>>, statbuf: u64) -> SyscallResult {
    let node = inode.lock();
    unsafe {
        core::ptr::write_bytes(statbuf as *mut u8, 0, 128);
        let type_bits: u32 = match node.itype {
            InodeType::File => 0o100000,
            InodeType::Directory => 0o040000,
            InodeType::CharDevice => 0o020000,
            _ => 0o100000,
        };
        *((statbuf + 16) as *mut u32) = type_bits | node.mode;
        *((statbuf + 48) as *mut u64) = node.size as u64;
        *((statbuf + 56) as *mut u64) = 4096;
    }
    Ok(0)
}

pub fn mkdir(path: &str, mode: u32) -> SyscallResult {
    let (parent_path, name) = rsplit_path(path);
    let parent = resolve_path(parent_path)?;
    let mut parent_node = parent.lock();

    if parent_node.children.contains_key(name) {
        return Err(EEXIST);
    }

    parent_node.children.insert(name.to_string(), Arc::new(Mutex::new(Inode::new_dir(mode))));
    Ok(0)
}

pub fn unlink(path: &str, flags: u32) -> SyscallResult {
    let (parent_path, name) = rsplit_path(path);
    let parent = resolve_path(parent_path)?;
    let mut parent_node = parent.lock();

    if parent_node.children.remove(name).is_some() {
        Ok(0)
    } else {
        Err(ENOENT)
    }
}

fn rsplit_path(path: &str) -> (&str, &str) {
    let path = path.trim_end_matches('/');
    match path.rfind('/') {
        Some(pos) => {
            let parent = if pos == 0 { "/" } else { &path[..pos] };
            (&parent, &path[pos + 1..])
        }
        None => ("/", path),
    }
}
