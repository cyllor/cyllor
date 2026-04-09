use super::{SyscallResult, EBADF, ENOSYS, EINVAL, ENOTDIR};
use alloc::string::ToString;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct IovecRaw {
    base: u64,
    len: u64,
}

fn read_iovec(iov: u64, index: u32) -> Result<IovecRaw, i32> {
    let mut raw = IovecRaw::default();
    let buf = unsafe {
        core::slice::from_raw_parts_mut(
            (&mut raw as *mut IovecRaw).cast::<u8>(),
            core::mem::size_of::<IovecRaw>(),
        )
    };
    copy_from_user(iov + (index as u64) * 16, buf)?;
    Ok(raw)
}

pub fn sys_write(fd: u64, buf: u64, count: u64) -> SyscallResult {
    // Always translate user VA → kernel VA via page table walk
    let ttbr0 = crate::arch::read_user_page_table_root();
    let hhdm = crate::arch::hhdm_offset();

    // Translate user buffer page-by-page and write
    let mut written = 0usize;
    let mut uaddr = buf;
    while written < count as usize {
        let pa = match crate::arch::translate_user_va(ttbr0, uaddr) {
            Some(pa) => pa,
            None => return if written > 0 { Ok(written) } else { Err(super::EFAULT) },
        };
        let kva = pa + hhdm;
        let page_remaining = 4096 - (kva & 0xFFF) as usize;
        let chunk = page_remaining.min(count as usize - written);
        let n = match crate::fs::fd_write(fd as u32, kva, chunk) {
            Ok(n) => n,
            Err(errno) => {
                if written > 0 {
                    return Ok(written);
                }
                return Err(errno);
            }
        };
        written += n;
        uaddr += n as u64;
        if n < chunk {
            break;
        }
    }
    Ok(written)
}

pub fn sys_read(fd: u64, buf: u64, count: u64) -> SyscallResult {
    crate::fs::fd_read(fd as u32, buf, count as usize)
}

pub fn sys_openat(dirfd: i32, pathname: u64, flags: u32, mode: u32) -> SyscallResult {
    let mut path_buf = [0u8; 256];
    let len = read_user_string(pathname, &mut path_buf)?;
    let path = core::str::from_utf8(&path_buf[..len]).unwrap_or("");
    crate::fs::openat(dirfd, path, flags, mode)
}

/// Read a null-terminated string from user memory, demand-paging as needed
fn read_user_string(addr: u64, buf: &mut [u8]) -> Result<usize, i32> {
    let ttbr0 = crate::arch::read_user_page_table_root();
    let hhdm = crate::arch::hhdm_offset();

    let mut len = 0;
    let mut uaddr = addr;
    while len < buf.len() - 1 {
        let pa = match crate::arch::translate_user_va(ttbr0, uaddr) {
            Some(pa) => pa,
            None => {
                // Unmapped page — treat as NUL terminator (end of string)
                break;
            }
        };
        let kva = pa + hhdm;
        let b = unsafe { *(kva as *const u8) };
        if b == 0 { break; }
        buf[len] = b;
        len += 1;
        uaddr += 1;
    }
    Ok(len)
}

pub fn sys_close(fd: u32) -> SyscallResult {
    crate::fs::close(fd)
}

pub fn sys_lseek(fd: u32, offset: i64, whence: u32) -> SyscallResult {
    crate::fs::lseek(fd, offset, whence)
}

pub fn sys_fstat(fd: u32, statbuf: u64) -> SyscallResult {
    crate::fs::fstat(fd, statbuf)
}

pub fn sys_newfstatat(dirfd: i32, pathname: u64, statbuf: u64, flags: u32) -> SyscallResult {
    let mut path_buf = [0u8; 256];
    let len = read_user_string(pathname, &mut path_buf)?;
    let path = core::str::from_utf8(&path_buf[..len]).unwrap_or("");
    crate::fs::fstatat(dirfd, path, statbuf, flags)
}

pub fn sys_getcwd(buf: u64, size: u64) -> SyscallResult {
    crate::fs::getcwd(buf, size as usize)
}

pub fn sys_chdir(path: u64) -> SyscallResult {
    let p = unsafe { cstr_from_user(path)? };
    crate::fs::chdir(p)
}

pub fn sys_mkdirat(dirfd: i32, pathname: u64, mode: u32) -> SyscallResult {
    let path = unsafe { cstr_from_user(pathname)? };
    crate::fs::mkdirat(dirfd, path, mode)
}

pub fn sys_unlinkat(dirfd: i32, pathname: u64, flags: u32) -> SyscallResult {
    let path = unsafe { cstr_from_user(pathname)? };
    crate::fs::unlinkat(dirfd, path, flags)
}

pub fn sys_renameat2(olddirfd: i32, oldpath: u64, newdirfd: i32, newpath: u64, flags: u32) -> SyscallResult {
    let oldp = unsafe { cstr_from_user(oldpath)? };
    let newp = unsafe { cstr_from_user(newpath)? };
    crate::fs::renameat2(olddirfd, oldp, newdirfd, newp, flags)
}

pub fn sys_faccessat(dirfd: i32, pathname: u64, mode: u32, flags: u32) -> SyscallResult {
    let path = unsafe { cstr_from_user(pathname)? };
    crate::fs::faccessat(dirfd, path, mode, flags)
}

pub fn sys_access(pathname: u64, mode: u32) -> SyscallResult {
    // access(path, mode) behaves like faccessat(AT_FDCWD, path, mode, 0)
    const AT_FDCWD: i32 = -100;
    let path = unsafe { cstr_from_user(pathname)? };
    crate::fs::faccessat(AT_FDCWD, path, mode, 0)
}

pub fn sys_statx(dirfd: i32, pathname: u64, flags: u32, mask: u32, statxbuf: u64) -> SyscallResult {
    let path = unsafe { cstr_from_user(pathname)? };
    crate::fs::statx(dirfd, path, flags, mask, statxbuf)
}

pub fn sys_writev(fd: u32, iov: u64, iovcnt: u32) -> SyscallResult {
    let mut total = 0usize;
    for i in 0..iovcnt {
        let ent = read_iovec(iov, i)?;
        if ent.len > 0 {
            total += sys_write(fd as u64, ent.base, ent.len)?;
        }
    }
    Ok(total)
}

pub fn sys_readv(fd: u32, iov: u64, iovcnt: u32) -> SyscallResult {
    let mut total = 0usize;
    for i in 0..iovcnt {
        let ent = read_iovec(iov, i)?;
        if ent.len > 0 {
            total += sys_read(fd as u64, ent.base, ent.len)?;
        }
    }
    Ok(total)
}

pub fn sys_dup(oldfd: u32) -> SyscallResult {
    crate::fs::dup(oldfd)
}

pub fn sys_dup3(oldfd: u32, newfd: u32, flags: u32) -> SyscallResult {
    crate::fs::dup3(oldfd, newfd, flags)
}

pub fn sys_pipe2(pipefd: u64, flags: u32) -> SyscallResult {
    crate::fs::pipe2(pipefd, flags)
}

pub fn sys_fcntl(fd: u32, cmd: u32, arg: u64) -> SyscallResult {
    crate::fs::fcntl(fd, cmd, arg)
}

/// Write kernel data to a user virtual address via page table translation
pub fn copy_to_user(user_addr: u64, data: &[u8]) -> Result<(), i32> {
    let ttbr0 = crate::arch::read_user_page_table_root();
    let hhdm = crate::arch::hhdm_offset();

    let mut offset = 0;
    while offset < data.len() {
        let uaddr = user_addr + offset as u64;
        match crate::arch::translate_user_va(ttbr0, uaddr) {
            Some(pa) => {
                let kva = pa + hhdm;
                let page_rem = 4096 - (uaddr & 0xFFF) as usize;
                let chunk = page_rem.min(data.len() - offset);
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        data[offset..].as_ptr(),
                        kva as *mut u8,
                        chunk,
                    );
                }
                offset += chunk;
            }
            None => {
                // Page fault — try to demand-page it
                let page_addr = uaddr & !0xFFF;
                let phys = crate::mm::pmm::alloc_page().ok_or(super::ENOMEM)? as u64;
                unsafe { core::ptr::write_bytes((phys + hhdm) as *mut u8, 0, 4096); }
                crate::arch::map_user_page(
                    ttbr0, page_addr, phys,
                    crate::arch::PageAttr::USER_RW,
                );
                // Retry this offset
            }
        }
    }
    Ok(())
}

/// Read from user virtual address to kernel buffer
pub fn copy_from_user(user_addr: u64, buf: &mut [u8]) -> Result<(), i32> {
    let ttbr0 = crate::arch::read_user_page_table_root();
    let hhdm = crate::arch::hhdm_offset();

    let mut offset = 0;
    while offset < buf.len() {
        let uaddr = user_addr + offset as u64;
        match crate::arch::translate_user_va(ttbr0, uaddr) {
            Some(pa) => {
                let kva = pa + hhdm;
                let page_rem = 4096 - (uaddr & 0xFFF) as usize;
                let chunk = page_rem.min(buf.len() - offset);
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        kva as *const u8,
                        buf[offset..].as_mut_ptr(),
                        chunk,
                    );
                }
                offset += chunk;
            }
            None => return Err(super::EFAULT),
        }
    }
    Ok(())
}

pub fn print_hex(val: u64) {
    for i in (0..16).rev() {
        let nibble = ((val >> (i * 4)) & 0xF) as u8;
        let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
        crate::drivers::uart::write_byte(c);
    }
}

pub fn sys_getdents64(fd: u32, dirp: u64, count: u32) -> SyscallResult {
    if dirp == 0 || count == 0 {
        return Err(EINVAL);
    }

    const DT_UNKNOWN: u8 = 0;
    const DT_FIFO: u8 = 1;
    const DT_CHR: u8 = 2;
    const DT_DIR: u8 = 4;
    const DT_BLK: u8 = 6;
    const DT_REG: u8 = 8;
    const DT_LNK: u8 = 10;
    const DT_SOCK: u8 = 12;

    fn align8(v: usize) -> usize {
        (v + 7) & !7
    }

    fn inode_type_to_dtype(t: crate::fs::vfs::InodeType) -> u8 {
        match t {
            crate::fs::vfs::InodeType::Directory => DT_DIR,
            crate::fs::vfs::InodeType::File => DT_REG,
            crate::fs::vfs::InodeType::CharDevice => DT_CHR,
            crate::fs::vfs::InodeType::BlockDevice => DT_BLK,
            crate::fs::vfs::InodeType::Pipe => DT_FIFO,
            crate::fs::vfs::InodeType::Socket => DT_SOCK,
            crate::fs::vfs::InodeType::Symlink => DT_LNK,
        }
    }

    let file = crate::fs::fdtable::get_file(fd)?;
    let mut f = file.lock();
    if f.ftype != crate::fs::vfs::FileType::Directory {
        return Err(ENOTDIR);
    }
    let inode = f.inode.clone().ok_or(EINVAL)?;
    let entries: alloc::vec::Vec<(alloc::string::String, u8)> = {
        let node = inode.lock();
        let mut out = alloc::vec![
            (".".to_string(), DT_DIR),
            ("..".to_string(), DT_DIR),
        ];
        out.extend(node.children.iter().map(|(name, child)| {
            let ty = inode_type_to_dtype(child.lock().itype);
            (name.clone(), ty)
        }));
        out
    };

    let mut written = 0usize;
    let mut idx = f.offset;
    let max = count as usize;
    while idx < entries.len() {
        let (name, dtype) = &entries[idx];
        let name_bytes = name.as_bytes();
        let reclen = align8(19 + name_bytes.len() + 1);
        if written + reclen > max {
            break;
        }

        let mut rec = alloc::vec![0u8; reclen];
        // linux_dirent64
        // 0..8: d_ino, 8..16: d_off, 16..18: d_reclen, 18: d_type, 19..: d_name\0
        rec[0..8].copy_from_slice(&((idx + 1) as u64).to_le_bytes());
        rec[8..16].copy_from_slice(&((idx + 1) as i64).to_le_bytes());
        rec[16..18].copy_from_slice(&(reclen as u16).to_le_bytes());
        rec[18] = *dtype;
        rec[19..19 + name_bytes.len()].copy_from_slice(name_bytes);
        rec[19 + name_bytes.len()] = 0;

        copy_to_user(dirp + written as u64, &rec)?;
        written += reclen;
        idx += 1;
    }

    f.offset = idx;
    Ok(written)
}

pub fn sys_readlinkat(dirfd: i32, pathname: u64, buf: u64, bufsiz: u32) -> SyscallResult {
    let mut path_buf = [0u8; 256];
    let len = read_user_string(pathname, &mut path_buf)?;
    let path = core::str::from_utf8(&path_buf[..len]).unwrap_or("");

    // Use resolve_path_lstat to get the symlink inode without following it
    match crate::fs::vfs::resolve_path_lstat(path) {
        Ok(node) => {
            let n = node.lock();
            if n.itype == crate::fs::vfs::InodeType::Symlink {
                let link_data = &n.data;
                let copy_len = link_data.len().min(bufsiz as usize);
                unsafe {
                    core::ptr::copy_nonoverlapping(link_data.as_ptr(), buf as *mut u8, copy_len);
                }
                return Ok(copy_len);
            }
            Err(EINVAL) // not a symlink
        }
        Err(e) => Err(e),
    }
}

unsafe fn cstr_from_user(ptr: u64) -> Result<&'static str, i32> {
    if ptr == 0 {
        return Err(EINVAL);
    }
    let mut len = 0;
    loop {
        if unsafe { *((ptr + len) as *const u8) } == 0 {
            break;
        }
        len += 1;
        if len > 4096 {
            return Err(EINVAL);
        }
    }
    let slice = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    core::str::from_utf8(slice).map_err(|_| EINVAL)
}
