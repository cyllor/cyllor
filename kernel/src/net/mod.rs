use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use crate::syscall::{SyscallResult, EAFNOSUPPORT, EAGAIN, EBADF, EFAULT, EINVAL, ENOTCONN, ENOTSOCK};

#[derive(Clone, Copy, Default)]
struct SocketState {
    domain: i32,
    sock_type: i32,
    protocol: i32,
    bound: bool,
    connected: bool,
    listening: bool,
    backlog: usize,
    local_addr: u32,
    local_port: u16,
    peer_addr: u32,
    peer_port: u16,
    peer_owner: i32,
    last_error: i32,
    creator_pid: u64,
    creator_uid: u32,
    creator_gid: u32,
}

// owner-fd keyed state
static SOCKET_STATE: Mutex<BTreeMap<i32, SocketState>> = Mutex::new(BTreeMap::new());
static SOCKET_PENDING: Mutex<BTreeMap<i32, Vec<i32>>> = Mutex::new(BTreeMap::new()); // listener-owner -> pending client-owner
static SOCKET_RX: Mutex<BTreeMap<i32, Vec<u8>>> = Mutex::new(BTreeMap::new()); // owner -> recv queue
static SOCKET_ALIAS: Mutex<BTreeMap<i32, i32>> = Mutex::new(BTreeMap::new()); // fd -> owner
static UNIX_PATH_OWNER: Mutex<BTreeMap<Vec<u8>, i32>> = Mutex::new(BTreeMap::new()); // sun_path -> owner
static OWNER_UNIX_PATH: Mutex<BTreeMap<i32, Vec<u8>>> = Mutex::new(BTreeMap::new()); // owner -> sun_path
static SOCKET_RIGHTS: Mutex<BTreeMap<i32, Vec<Arc<spin::Mutex<crate::fs::vfs::FileObject>>>>> =
    Mutex::new(BTreeMap::new()); // owner -> pending passed fds

const AF_UNIX: i32 = 1;
const AF_INET: i32 = 2;

const POLLIN: u32 = 0x0001;
const POLLOUT: u32 = 0x0004;
const POLLERR: u32 = 0x0008;
const POLLHUP: u32 = 0x0010;

const SOL_SOCKET: i32 = 1;
const SO_TYPE: i32 = 3;
const SO_ERROR: i32 = 4;
const SO_PEERCRED: i32 = 17;
const SO_ACCEPTCONN: i32 = 30;
const SCM_RIGHTS: i32 = 1;
const SCM_CREDENTIALS: i32 = 2;
const MSG_CTRUNC: u32 = 0x8;

fn read_sockaddr_in(addr: u64, addrlen: u32) -> Result<(u16, u32), i32> {
    if addr == 0 || addrlen < 8 {
        return Err(EINVAL);
    }
    let mut raw = [0u8; 16];
    crate::syscall::fs::copy_from_user(addr, &mut raw).map_err(|_| EFAULT)?;
    let family = u16::from_le_bytes([raw[0], raw[1]]);
    if family != AF_INET as u16 {
        return Err(EAFNOSUPPORT);
    }
    let port = u16::from_be_bytes([raw[2], raw[3]]);
    let ip = u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]);
    Ok((port, ip))
}

fn read_sockaddr_un_path(addr: u64, addrlen: u32) -> Result<Vec<u8>, i32> {
    if addr == 0 || addrlen < 3 {
        return Err(EINVAL);
    }
    let mut raw = vec![0u8; addrlen as usize];
    crate::syscall::fs::copy_from_user(addr, &mut raw).map_err(|_| EFAULT)?;
    let family = u16::from_le_bytes([raw[0], raw[1]]);
    if family != AF_UNIX as u16 {
        return Err(EAFNOSUPPORT);
    }
    let path = &raw[2..];
    if path.is_empty() {
        return Err(EINVAL);
    }
    if path[0] == 0 {
        // Abstract UNIX path (keep leading NUL and full payload).
        return Ok(path.to_vec());
    }
    let nul = path.iter().position(|b| *b == 0).unwrap_or(path.len());
    if nul == 0 {
        return Err(EINVAL);
    }
    Ok(path[..nul].to_vec())
}

fn write_sockaddr_in(addr: u64, addrlen_ptr: u64, port: u16, ip: u32) -> Result<(), i32> {
    if addrlen_ptr == 0 {
        return Err(EFAULT);
    }

    let mut len_raw = [0u8; 4];
    crate::syscall::fs::copy_from_user(addrlen_ptr, &mut len_raw).map_err(|_| EFAULT)?;
    let mut user_len = u32::from_le_bytes(len_raw);

    let mut sa = [0u8; 16];
    sa[0..2].copy_from_slice(&(AF_INET as u16).to_le_bytes());
    sa[2..4].copy_from_slice(&port.to_be_bytes());
    sa[4..8].copy_from_slice(&ip.to_be_bytes());

    if addr != 0 && user_len > 0 {
        let copy_len = core::cmp::min(user_len as usize, sa.len());
        crate::syscall::fs::copy_to_user(addr, &sa[..copy_len]).map_err(|_| EFAULT)?;
    }
    user_len = sa.len() as u32;
    crate::syscall::fs::copy_to_user(addrlen_ptr, &user_len.to_le_bytes()).map_err(|_| EFAULT)?;
    Ok(())
}

fn ensure_socket_fd(fd: i32) -> Result<(), i32> {
    if fd < 0 {
        return Err(EBADF);
    }
    let file = crate::fs::fdtable::get_file(fd as u32)?;
    if file.lock().ftype != crate::fs::vfs::FileType::Socket {
        return Err(ENOTSOCK);
    }
    Ok(())
}

fn owner_of(fd: i32) -> i32 {
    SOCKET_ALIAS.lock().get(&fd).copied().unwrap_or(fd)
}

fn set_socket_error(fd: i32, err: i32) {
    let owner = owner_of(fd);
    if let Some(st) = SOCKET_STATE.lock().get_mut(&owner) {
        st.last_error = err;
    }
}

fn is_nonblocking_fd(fd: i32) -> bool {
    if fd < 0 {
        return true;
    }
    if let Ok(file) = crate::fs::fdtable::get_file(fd as u32) {
        return (file.lock().flags & 0o4000) != 0;
    }
    true
}

fn alloc_socket_fd(domain: i32, sock_type: i32, protocol: i32) -> Result<i32, i32> {
    let creator_pid = crate::sched::process::current_pid();
    let (uid, gid, euid, egid) = crate::syscall::current_creds();
    let creator_uid = if euid != 0 { euid } else { uid };
    let creator_gid = if egid != 0 { egid } else { gid };
    let file = alloc::sync::Arc::new(spin::Mutex::new(crate::fs::vfs::FileObject {
        inode: None,
        offset: 0,
        flags: 0,
        ftype: crate::fs::vfs::FileType::Socket,
        special_data: None,
    }));
    let fd = crate::fs::alloc_fd(file)? as i32;
    SOCKET_ALIAS.lock().insert(fd, fd);
    SOCKET_STATE.lock().insert(
        fd,
        SocketState {
            domain,
            sock_type,
            protocol,
            peer_owner: -1,
            creator_pid,
            creator_uid,
            creator_gid,
            ..SocketState::default()
        },
    );
    SOCKET_PENDING.lock().entry(fd).or_default();
    SOCKET_RX.lock().entry(fd).or_default();
    Ok(fd)
}

pub fn socket_poll_mask(fd: i32) -> u32 {
    if ensure_socket_fd(fd).is_err() {
        return POLLERR | POLLHUP;
    }
    let owner = owner_of(fd);
    let mut mask = POLLOUT;
    if SOCKET_RX.lock().get(&owner).map_or(false, |q| !q.is_empty()) {
        mask |= POLLIN;
    }
    if SOCKET_PENDING.lock().get(&owner).map_or(false, |q| !q.is_empty()) {
        mask |= POLLIN;
    }
    if SOCKET_STATE
        .lock()
        .get(&owner)
        .map_or(false, |s| !s.listening && s.peer_owner < 0 && !s.connected)
    {
        mask |= POLLHUP;
    }
    mask
}

pub fn cleanup_socket_fd(fd: i32) {
    let owner = owner_of(fd);
    SOCKET_ALIAS.lock().remove(&fd);

    let has_alias = SOCKET_ALIAS.lock().values().any(|&v| v == owner);
    if has_alias {
        return;
    }

    SOCKET_STATE.lock().remove(&owner);
    SOCKET_PENDING.lock().remove(&owner);
    SOCKET_RX.lock().remove(&owner);
    SOCKET_RIGHTS.lock().remove(&owner);
    if let Some(path) = OWNER_UNIX_PATH.lock().remove(&owner) {
        UNIX_PATH_OWNER.lock().remove(&path);
    }

    {
        let mut state = SOCKET_STATE.lock();
        for s in state.values_mut() {
            if s.peer_owner == owner {
                s.peer_owner = -1;
                s.connected = false;
            }
        }
    }
    let mut pending = SOCKET_PENDING.lock();
    for q in pending.values_mut() {
        q.retain(|p| *p != owner);
    }
}

pub fn clone_socket_fd(oldfd: i32, newfd: i32) {
    let owner = owner_of(oldfd);
    SOCKET_ALIAS.lock().insert(newfd, owner);
}

pub fn do_socket(domain: i32, sock_type: i32, protocol: i32) -> SyscallResult {
    if domain != AF_INET && domain != AF_UNIX {
        return Err(EAFNOSUPPORT);
    }
    Ok(alloc_socket_fd(domain, sock_type, protocol)? as usize)
}

pub fn do_bind(fd: i32, addr: u64, addrlen: u32) -> SyscallResult {
    ensure_socket_fd(fd)?;
    let owner = owner_of(fd);
    let domain = SOCKET_STATE.lock().get(&owner).map(|s| s.domain).ok_or(EBADF)?;
    if domain == AF_UNIX {
        let path = read_sockaddr_un_path(addr, addrlen)?;
        {
            let map = UNIX_PATH_OWNER.lock();
            if map.get(&path).copied().map_or(false, |v| v != owner) {
                set_socket_error(fd, crate::syscall::EADDRINUSE);
                return Err(crate::syscall::EADDRINUSE);
            }
        }
        UNIX_PATH_OWNER.lock().insert(path.clone(), owner);
        OWNER_UNIX_PATH.lock().insert(owner, path);
        let mut states = SOCKET_STATE.lock();
        let st = states.get_mut(&owner).ok_or(EBADF)?;
        st.bound = true;
        st.last_error = 0;
        return Ok(0);
    }
    let (port, ip) = read_sockaddr_in(addr, addrlen)?;
    let in_use = {
        let states = SOCKET_STATE.lock();
        states.iter().any(|(k, s)| {
            *k != owner && s.bound && s.local_port == port && (s.local_addr == ip || s.local_addr == 0 || ip == 0)
        })
    };
    if in_use {
        set_socket_error(fd, crate::syscall::EADDRINUSE);
        return Err(crate::syscall::EADDRINUSE);
    }
    let mut states = SOCKET_STATE.lock();
    let st = states.get_mut(&owner).ok_or(EBADF)?;
    st.bound = true;
    st.local_port = port;
    st.local_addr = ip;
    st.last_error = 0;
    Ok(0)
}

pub fn do_listen(fd: i32, backlog: i32) -> SyscallResult {
    ensure_socket_fd(fd)?;
    let owner = owner_of(fd);
    let mut states = SOCKET_STATE.lock();
    let st = states.get_mut(&owner).ok_or(EBADF)?;
    if !st.bound {
        return Err(EINVAL);
    }
    st.listening = true;
    st.backlog = backlog.max(1) as usize;
    st.last_error = 0;
    SOCKET_PENDING.lock().entry(owner).or_default();
    Ok(0)
}

pub fn do_accept4(fd: i32, addr: u64, addrlen: u64, _flags: u32) -> SyscallResult {
    ensure_socket_fd(fd)?;
    let listener_owner = owner_of(fd);
    let listener = SOCKET_STATE.lock().get(&listener_owner).copied().ok_or(EBADF)?;
    if !listener.listening {
        return Err(EINVAL);
    }

    let nonblock = is_nonblocking_fd(fd) || ((_flags & 0x800) != 0);
    let client_owner = loop {
        let pending = SOCKET_PENDING.lock();
        let queue = pending.get(&listener_owner).ok_or(EBADF)?;
        if let Some(&c) = queue.first() {
            break c;
        }
        drop(pending);
        if nonblock {
            set_socket_error(fd, EAGAIN);
            return Err(EAGAIN);
        }
        crate::sched::wait::sleep_ticks(1);
    };

    let accepted_fd = alloc_socket_fd(listener.domain, listener.sock_type, listener.protocol)?;
    if (_flags & 0x800) != 0 {
        if let Ok(f) = crate::fs::fdtable::get_file(accepted_fd as u32) {
            f.lock().flags |= 0o4000;
        }
    }
    let accepted_owner = owner_of(accepted_fd);

    {
        let mut pending = SOCKET_PENDING.lock();
        if let Some(queue) = pending.get_mut(&listener_owner) {
            if let Some(pos) = queue.iter().position(|x| *x == client_owner) {
                queue.remove(pos);
            } else {
                cleanup_socket_fd(accepted_fd);
                return Err(EAGAIN);
            }
        }
    }

    {
        let mut states = SOCKET_STATE.lock();
        let client = states.get(&client_owner).copied().ok_or(EBADF)?;
        if let Some(acc) = states.get_mut(&accepted_owner) {
            acc.bound = true;
            acc.connected = true;
            acc.local_addr = listener.local_addr;
            acc.local_port = listener.local_port;
            acc.peer_addr = client.local_addr;
            acc.peer_port = client.local_port;
            acc.peer_owner = client_owner;
        }
        if let Some(cli) = states.get_mut(&client_owner) {
            cli.connected = true;
            cli.peer_addr = listener.local_addr;
            cli.peer_port = listener.local_port;
            cli.peer_owner = accepted_owner;
        }
    }

    if addr != 0 && addrlen != 0 {
        let client = SOCKET_STATE.lock().get(&client_owner).copied().ok_or(EBADF)?;
        write_sockaddr_in(addr, addrlen, client.local_port, client.local_addr)?;
    }
    set_socket_error(fd, 0);
    Ok(accepted_fd as usize)
}

pub fn do_connect(fd: i32, addr: u64, addrlen: u32) -> SyscallResult {
    ensure_socket_fd(fd)?;
    let owner = owner_of(fd);
    let domain = SOCKET_STATE.lock().get(&owner).map(|s| s.domain).ok_or(EBADF)?;
    let already_connected = {
        let states = SOCKET_STATE.lock();
        states.get(&owner).map_or(false, |s| s.connected)
    };
    if already_connected {
        set_socket_error(fd, crate::syscall::EISCONN);
        return Err(crate::syscall::EISCONN);
    }

    if domain == AF_UNIX {
        let path = read_sockaddr_un_path(addr, addrlen)?;
        let listener_owner = UNIX_PATH_OWNER.lock().get(&path).copied().ok_or(crate::syscall::ECONNREFUSED)?;
        if listener_owner == owner {
            set_socket_error(fd, EINVAL);
            return Err(EINVAL);
        }
        let listener = SOCKET_STATE.lock().get(&listener_owner).copied().ok_or(EBADF)?;
        if !listener.listening {
            set_socket_error(fd, crate::syscall::ECONNREFUSED);
            return Err(crate::syscall::ECONNREFUSED);
        }
        let backlog = listener.backlog.max(1);
        let backlog_full = {
            let mut pending = SOCKET_PENDING.lock();
            let q = pending.entry(listener_owner).or_default();
            if q.iter().any(|&p| p == owner) {
                return Ok(0);
            }
            if q.len() >= backlog {
                true
            } else {
                q.push(owner);
                false
            }
        };
        if backlog_full {
            set_socket_error(fd, EAGAIN);
            return Err(EAGAIN);
        }
        let mut states = SOCKET_STATE.lock();
        let st = states.get_mut(&owner).ok_or(EBADF)?;
        st.connected = true;
        st.last_error = 0;
        return Ok(0);
    }

    let (peer_port, peer_ip) = read_sockaddr_in(addr, addrlen)?;

    let listener_owner = {
        let states = SOCKET_STATE.lock();
        match states
            .iter()
            .find_map(|(k, st)| {
                if st.listening
                    && st.bound
                    && st.local_port == peer_port
                    && (st.local_addr == peer_ip || st.local_addr == 0 || peer_ip == 0)
                {
                    Some(*k)
                } else {
                    None
                }
            }) {
            Some(v) => v,
            None => {
                set_socket_error(fd, crate::syscall::ECONNREFUSED);
                return Err(crate::syscall::ECONNREFUSED);
            }
        }
    };
    if listener_owner == owner {
        set_socket_error(fd, EINVAL);
        return Err(EINVAL);
    }

    let backlog = SOCKET_STATE.lock().get(&listener_owner).map(|s| s.backlog).unwrap_or(1);
    let backlog_full = {
        let mut pending = SOCKET_PENDING.lock();
        let q = pending.entry(listener_owner).or_default();
        if q.iter().any(|&p| p == owner) {
            return Ok(0);
        }
        if q.len() >= backlog {
            true
        } else {
            q.push(owner);
            false
        }
    };
    if backlog_full {
        set_socket_error(fd, EAGAIN);
        return Err(EAGAIN);
    }

    {
        let mut states = SOCKET_STATE.lock();
        let st = states.get_mut(&owner).ok_or(EBADF)?;
        st.connected = true;
        st.peer_port = peer_port;
        st.peer_addr = peer_ip;
        if !st.bound {
            st.bound = true;
            st.local_port = 49152u16.saturating_add((owner as u16) & 0x0fff);
            st.local_addr = 0;
        }
        st.last_error = 0;
    }
    Ok(0)
}

pub fn do_socketpair(domain: i32, sock_type: i32, protocol: i32, sv: u64) -> SyscallResult {
    if domain != AF_UNIX {
        return Err(EAFNOSUPPORT);
    }
    if sv == 0 {
        return Err(EFAULT);
    }
    let fd0 = alloc_socket_fd(domain, sock_type, protocol)?;
    let fd1 = alloc_socket_fd(domain, sock_type, protocol)?;
    let o0 = owner_of(fd0);
    let o1 = owner_of(fd1);
    {
        let mut states = SOCKET_STATE.lock();
        if let Some(s0) = states.get_mut(&o0) {
            s0.connected = true;
            s0.bound = true;
            s0.peer_owner = o1;
        }
        if let Some(s1) = states.get_mut(&o1) {
            s1.connected = true;
            s1.bound = true;
            s1.peer_owner = o0;
        }
    }
    let mut out = [0u8; 8];
    out[0..4].copy_from_slice(&(fd0 as i32).to_le_bytes());
    out[4..8].copy_from_slice(&(fd1 as i32).to_le_bytes());
    crate::syscall::fs::copy_to_user(sv, &out).map_err(|_| EFAULT)?;
    Ok(0)
}

fn read_msghdr(msghdr: u64) -> Result<(u64, usize, u64, usize, u64, usize), i32> {
    if msghdr == 0 {
        return Err(EFAULT);
    }
    let mut raw = [0u8; 56];
    crate::syscall::fs::copy_from_user(msghdr, &mut raw).map_err(|_| EFAULT)?;
    let msg_name = u64::from_le_bytes(raw[0..8].try_into().unwrap_or([0; 8]));
    let msg_namelen = u32::from_le_bytes(raw[8..12].try_into().unwrap_or([0; 4])) as usize;
    let msg_iov = u64::from_le_bytes(raw[16..24].try_into().unwrap_or([0; 8]));
    let msg_iovlen = u64::from_le_bytes(raw[24..32].try_into().unwrap_or([0; 8])) as usize;
    let msg_control = u64::from_le_bytes(raw[32..40].try_into().unwrap_or([0; 8]));
    let msg_controllen = u64::from_le_bytes(raw[40..48].try_into().unwrap_or([0; 8])) as usize;
    Ok((msg_name, msg_namelen, msg_iov, msg_iovlen, msg_control, msg_controllen))
}

fn collect_iovecs(msg_iov: u64, msg_iovlen: usize, max_total: usize) -> Result<Vec<Vec<u8>>, i32> {
    if msg_iovlen == 0 {
        return Ok(Vec::new());
    }
    let capped = core::cmp::min(msg_iovlen, 128);
    let mut out = Vec::new();
    let mut total = 0usize;
    for i in 0..capped {
        let mut raw = [0u8; 16];
        crate::syscall::fs::copy_from_user(msg_iov + (i as u64) * 16, &mut raw).map_err(|_| EFAULT)?;
        let base = u64::from_le_bytes(raw[0..8].try_into().unwrap_or([0; 8]));
        let len = u64::from_le_bytes(raw[8..16].try_into().unwrap_or([0; 8])) as usize;
        if len == 0 {
            out.push(Vec::new());
            continue;
        }
        let take = core::cmp::min(len, max_total.saturating_sub(total));
        let mut buf = vec![0u8; take];
        crate::syscall::fs::copy_from_user(base, &mut buf).map_err(|_| EFAULT)?;
        total = total.saturating_add(take);
        out.push(buf);
        if total >= max_total {
            break;
        }
    }
    Ok(out)
}

fn scatter_to_iovecs(msg_iov: u64, msg_iovlen: usize, data: &[u8]) -> Result<usize, i32> {
    let capped = core::cmp::min(msg_iovlen, 128);
    let mut copied = 0usize;
    for i in 0..capped {
        if copied >= data.len() {
            break;
        }
        let mut raw = [0u8; 16];
        crate::syscall::fs::copy_from_user(msg_iov + (i as u64) * 16, &mut raw).map_err(|_| EFAULT)?;
        let base = u64::from_le_bytes(raw[0..8].try_into().unwrap_or([0; 8]));
        let len = u64::from_le_bytes(raw[8..16].try_into().unwrap_or([0; 8])) as usize;
        if len == 0 {
            continue;
        }
        let n = core::cmp::min(len, data.len() - copied);
        crate::syscall::fs::copy_to_user(base, &data[copied..copied + n]).map_err(|_| EFAULT)?;
        copied += n;
    }
    Ok(copied)
}

fn cmsg_align(len: usize) -> usize {
    let a = core::mem::size_of::<usize>();
    (len + (a - 1)) & !(a - 1)
}

pub fn do_sendmsg(fd: i32, msg: u64, _flags: u32) -> SyscallResult {
    ensure_socket_fd(fd)?;
    let owner = owner_of(fd);
    let st = SOCKET_STATE.lock().get(&owner).copied().ok_or(EBADF)?;
    if !st.connected || st.peer_owner < 0 {
        set_socket_error(fd, ENOTCONN);
        return Err(ENOTCONN);
    }
    let (_name, _namelen, msg_iov, msg_iovlen, msg_control, msg_controllen) = read_msghdr(msg)?;
    let iovs = collect_iovecs(msg_iov, msg_iovlen, 1 << 20)?;
    let mut payload = Vec::new();
    for part in iovs {
        payload.extend_from_slice(&part);
    }
    if !payload.is_empty() {
        SOCKET_RX.lock().entry(st.peer_owner).or_default().extend_from_slice(&payload);
    }

    if msg_control != 0 && msg_controllen >= 16 {
        let mut cmsg = vec![0u8; msg_controllen];
        crate::syscall::fs::copy_from_user(msg_control, &mut cmsg).map_err(|_| EFAULT)?;
        let mut off = 0usize;
        while off + 16 <= cmsg.len() {
            let cmsg_len = u64::from_le_bytes(cmsg[off..off + 8].try_into().unwrap_or([0; 8])) as usize;
            if cmsg_len < 16 || off + cmsg_len > cmsg.len() {
                break;
            }
            let cmsg_level = i32::from_le_bytes(cmsg[off + 8..off + 12].try_into().unwrap_or([0; 4]));
            let cmsg_type = i32::from_le_bytes(cmsg[off + 12..off + 16].try_into().unwrap_or([0; 4]));
            if cmsg_level == SOL_SOCKET && cmsg_type == SCM_RIGHTS {
                let data = &cmsg[off + 16..off + cmsg_len];
                let nfd = data.len() / 4;
                if nfd > 0 {
                    let mut rights = SOCKET_RIGHTS.lock();
                    let q = rights.entry(st.peer_owner).or_default();
                    for i in 0..nfd {
                        let p = i * 4;
                        let mut arr = [0u8; 4];
                        arr.copy_from_slice(&data[p..p + 4]);
                        let pass_fd = i32::from_le_bytes(arr);
                        if pass_fd < 0 {
                            continue;
                        }
                        if let Ok(file) = crate::fs::fdtable::get_file(pass_fd as u32) {
                            q.push(file);
                        }
                    }
                }
            } else if cmsg_level == SOL_SOCKET && cmsg_type == SCM_CREDENTIALS {
                // Accepted but ignored in this minimal model.
            }
            let step = cmsg_align(cmsg_len);
            if step == 0 {
                break;
            }
            off = off.saturating_add(step);
        }
    }
    set_socket_error(fd, 0);
    Ok(payload.len())
}

pub fn do_recvmsg(fd: i32, msg: u64, _flags: u32) -> SyscallResult {
    ensure_socket_fd(fd)?;
    let owner = owner_of(fd);
    let (_name, _namelen, msg_iov, msg_iovlen, msg_control, msg_controllen) = read_msghdr(msg)?;
    let out = loop {
        let mut rx = SOCKET_RX.lock();
        let q = rx.entry(owner).or_default();
        if !q.is_empty() {
            let out = q.clone();
            q.clear();
            drop(rx);
            break out;
        }
        drop(rx);
        if is_nonblocking_fd(fd) {
            set_socket_error(fd, EAGAIN);
            return Err(EAGAIN);
        }
        crate::sched::wait::sleep_ticks(1);
    };
    let copied = scatter_to_iovecs(msg_iov, msg_iovlen, &out)?;

    let mut used_control = 0usize;
    let mut msg_flags = 0u32;
    if msg_control != 0 && msg_controllen >= 16 {
        let mut rights_map = SOCKET_RIGHTS.lock();
        let pending = rights_map.entry(owner).or_default();
        if !pending.is_empty() {
            let max_fd = (msg_controllen.saturating_sub(16)) / 4;
            let take = core::cmp::min(max_fd, pending.len());
            if take > 0 {
                let cmsg_len = 16 + take * 4;
                let mut cbuf = vec![0u8; cmsg_len];
                cbuf[0..8].copy_from_slice(&(cmsg_len as u64).to_le_bytes());
                cbuf[8..12].copy_from_slice(&(SOL_SOCKET as i32).to_le_bytes());
                cbuf[12..16].copy_from_slice(&(SCM_RIGHTS as i32).to_le_bytes());
                for i in 0..take {
                    let file = pending[i].clone();
                    let newfd = crate::fs::fdtable::alloc_fd(file).unwrap_or(usize::MAX);
                    let fdv = if newfd == usize::MAX { -1i32 } else { newfd as i32 };
                    let off = 16 + i * 4;
                    cbuf[off..off + 4].copy_from_slice(&fdv.to_le_bytes());
                }
                crate::syscall::fs::copy_to_user(msg_control, &cbuf).map_err(|_| EFAULT)?;
                used_control = cmsg_len;
                pending.drain(..take);
            }
            if !pending.is_empty() {
                msg_flags |= MSG_CTRUNC;
            }
        }
    }

    let mut raw = [0u8; 56];
    crate::syscall::fs::copy_from_user(msg, &mut raw).map_err(|_| EFAULT)?;
    raw[8..12].copy_from_slice(&0u32.to_le_bytes()); // msg_namelen
    raw[40..48].copy_from_slice(&(used_control as u64).to_le_bytes()); // msg_controllen
    raw[48..52].copy_from_slice(&msg_flags.to_le_bytes()); // msg_flags
    crate::syscall::fs::copy_to_user(msg, &raw).map_err(|_| EFAULT)?;
    set_socket_error(fd, 0);
    Ok(copied)
}

pub fn do_sendto(fd: i32, buf: u64, len: u64, _flags: u32, _dest_addr: u64, _addrlen: u32) -> SyscallResult {
    ensure_socket_fd(fd)?;
    let owner = owner_of(fd);
    let st = SOCKET_STATE.lock().get(&owner).copied().ok_or(EBADF)?;
    if !st.connected || st.peer_owner < 0 {
        set_socket_error(fd, ENOTCONN);
        return Err(ENOTCONN);
    }
    let mut temp = vec![0u8; core::cmp::min(len as usize, 65536)];
    if !temp.is_empty() {
        crate::syscall::fs::copy_from_user(buf, &mut temp).map_err(|_| EFAULT)?;
        SOCKET_RX.lock().entry(st.peer_owner).or_default().extend_from_slice(&temp);
    }
    set_socket_error(fd, 0);
    Ok(temp.len())
}

pub fn do_recvfrom(fd: i32, buf: u64, len: u64, _flags: u32, src_addr: u64, addrlen: u64) -> SyscallResult {
    const MSG_PEEK: u32 = 0x2;
    ensure_socket_fd(fd)?;
    let owner = owner_of(fd);
    let out = loop {
        let mut rx = SOCKET_RX.lock();
        let q = rx.entry(owner).or_default();
        if !q.is_empty() {
            let to_read = core::cmp::min(len as usize, q.len());
            let out = q[..to_read].to_vec();
            if (_flags & MSG_PEEK) == 0 {
                q.drain(..to_read);
            }
            drop(rx);
            break out;
        }
        drop(rx);
        if is_nonblocking_fd(fd) {
            set_socket_error(fd, EAGAIN);
            return Err(EAGAIN);
        }
        crate::sched::wait::sleep_ticks(1);
    };

    crate::syscall::fs::copy_to_user(buf, &out).map_err(|_| EFAULT)?;
    if src_addr != 0 && addrlen != 0 {
        let st = SOCKET_STATE.lock().get(&owner).copied().ok_or(EBADF)?;
        if st.connected {
            write_sockaddr_in(src_addr, addrlen, st.peer_port, st.peer_addr)?;
        }
    }
    set_socket_error(fd, 0);
    Ok(out.len())
}

pub fn do_getsockname(fd: i32, addr: u64, addrlen_ptr: u64) -> SyscallResult {
    ensure_socket_fd(fd)?;
    if addr == 0 {
        return Err(EFAULT);
    }
    let owner = owner_of(fd);
    let st = SOCKET_STATE.lock().get(&owner).copied().ok_or(EBADF)?;
    write_sockaddr_in(addr, addrlen_ptr, st.local_port, st.local_addr)?;
    Ok(0)
}

pub fn do_getpeername(fd: i32, addr: u64, addrlen_ptr: u64) -> SyscallResult {
    ensure_socket_fd(fd)?;
    let owner = owner_of(fd);
    let st = SOCKET_STATE.lock().get(&owner).copied().ok_or(EBADF)?;
    if !st.connected {
        set_socket_error(fd, ENOTCONN);
        return Err(ENOTCONN);
    }
    if addr == 0 {
        return Err(EFAULT);
    }
    write_sockaddr_in(addr, addrlen_ptr, st.peer_port, st.peer_addr)?;
    Ok(0)
}

pub fn do_setsockopt(fd: i32, level: i32, _optname: i32, optval: u64, optlen: u32) -> SyscallResult {
    ensure_socket_fd(fd)?;
    if level != SOL_SOCKET {
        return Err(crate::syscall::EOPNOTSUPP);
    }
    if optval != 0 && optlen > 0 {
        let mut scratch = vec![0u8; core::cmp::min(optlen as usize, 16)];
        let _ = crate::syscall::fs::copy_from_user(optval, &mut scratch);
    }
    Ok(0)
}

pub fn do_getsockopt(fd: i32, level: i32, optname: i32, optval: u64, optlen: u64) -> SyscallResult {
    ensure_socket_fd(fd)?;
    if optlen == 0 {
        return Err(EFAULT);
    }
    let owner = owner_of(fd);
    if level == SOL_SOCKET && optname == SO_PEERCRED {
        let peer = SOCKET_STATE.lock().get(&owner).copied().ok_or(EBADF)?.peer_owner;
        if peer < 0 {
            return Err(ENOTCONN);
        }
        let peer_state = SOCKET_STATE.lock().get(&peer).copied().ok_or(ENOTCONN)?;
        let mut out = [0u8; 12];
        let pid = peer_state.creator_pid as u32;
        out[0..4].copy_from_slice(&pid.to_le_bytes()); // pid
        out[4..8].copy_from_slice(&peer_state.creator_uid.to_le_bytes()); // uid
        out[8..12].copy_from_slice(&peer_state.creator_gid.to_le_bytes()); // gid
        let mut len_raw = [0u8; 4];
        crate::syscall::fs::copy_from_user(optlen, &mut len_raw).map_err(|_| EFAULT)?;
        let user_len = u32::from_le_bytes(len_raw) as usize;
        let copy_len = core::cmp::min(user_len, out.len());
        if optval != 0 && copy_len > 0 {
            crate::syscall::fs::copy_to_user(optval, &out[..copy_len]).map_err(|_| EFAULT)?;
        }
        let out_len = out.len() as u32;
        crate::syscall::fs::copy_to_user(optlen, &out_len.to_le_bytes()).map_err(|_| EFAULT)?;
        return Ok(0);
    }
    let value = if level == SOL_SOCKET {
        match optname {
            SO_TYPE => SOCKET_STATE.lock().get(&owner).copied().ok_or(EBADF)?.sock_type,
            SO_ERROR => {
                let mut map = SOCKET_STATE.lock();
                let st = map.get_mut(&owner).ok_or(EBADF)?;
                let v = st.last_error;
                st.last_error = 0;
                v
            }
            SO_ACCEPTCONN => {
                if SOCKET_STATE.lock().get(&owner).copied().ok_or(EBADF)?.listening { 1 } else { 0 }
            }
            _ => 0,
        }
    } else {
        return Err(crate::syscall::EOPNOTSUPP);
    };

    let mut len_raw = [0u8; 4];
    crate::syscall::fs::copy_from_user(optlen, &mut len_raw).map_err(|_| EFAULT)?;
    let user_len = u32::from_le_bytes(len_raw);
    let out = value.to_le_bytes();
    let copy_len = core::cmp::min(user_len as usize, out.len());
    if optval != 0 && copy_len > 0 {
        crate::syscall::fs::copy_to_user(optval, &out[..copy_len]).map_err(|_| EFAULT)?;
    }
    let out_len = out.len() as u32;
    crate::syscall::fs::copy_to_user(optlen, &out_len.to_le_bytes()).map_err(|_| EFAULT)?;
    Ok(0)
}
