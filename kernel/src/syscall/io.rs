use super::{SyscallResult, EFAULT, EINVAL, POLLERR, POLLHUP, POLLIN, POLLNVAL, POLLOUT};

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

struct SigmaskGuard {
    old: Option<u64>,
}

impl Drop for SigmaskGuard {
    fn drop(&mut self) {
        if let Some(m) = self.old.take() {
            crate::ipc::signal::replace_current_sigmask(m);
        }
    }
}

fn fd_ready_mask(fd: i32, interest: i16) -> i16 {
    let Ok(f) = crate::fs::fdtable::get_file(fd as u32) else {
        return POLLNVAL as i16;
    };
    let file = f.lock();
    let mut ready = 0u32;
    match file.ftype {
        crate::fs::vfs::FileType::Regular
        | crate::fs::vfs::FileType::Directory
        | crate::fs::vfs::FileType::CharDevice
        | crate::fs::vfs::FileType::MemFd
        | crate::fs::vfs::FileType::PidFd => {
            ready |= POLLIN | POLLOUT;
        }
        crate::fs::vfs::FileType::Pipe => {
            if let Some(crate::fs::vfs::SpecialData::PipeBuffer(ref pipe_buf)) = file.special_data {
                if !pipe_buf.lock().is_empty() {
                    ready |= POLLIN;
                }
            }
            ready |= POLLOUT;
        }
        crate::fs::vfs::FileType::EventFd => {
            if let Some(crate::fs::vfs::SpecialData::EventFdVal(v)) = file.special_data {
                if v > 0 {
                    ready |= POLLIN;
                }
            }
            ready |= POLLOUT;
        }
        crate::fs::vfs::FileType::TimerFd => {
            if let Some(crate::fs::vfs::SpecialData::TimerFdState { next_deadline, pending_expirations, .. }) = file.special_data {
                if pending_expirations > 0 || (next_deadline != 0 && crate::arch::read_counter() >= next_deadline) {
                    ready |= POLLIN;
                }
            }
        }
        crate::fs::vfs::FileType::SignalFd => {
            if let Some(crate::fs::vfs::SpecialData::SignalFdMask(mask)) = file.special_data {
                if crate::ipc::signal::signalfd_ready(mask) {
                    ready |= POLLIN;
                }
            }
        }
        crate::fs::vfs::FileType::Socket => {
            ready |= crate::net::socket_poll_mask(fd);
        }
        crate::fs::vfs::FileType::PtyMaster | crate::fs::vfs::FileType::PtySlave => {
            if let Some(sd) = &file.special_data {
                let (id, is_master) = match sd {
                    crate::fs::vfs::SpecialData::PtyMaster(id) => (*id, true),
                    crate::fs::vfs::SpecialData::PtySlave(id) => (*id, false),
                    _ => (u32::MAX, true),
                };
                if id != u32::MAX {
                    if let Some(pty_arc) = crate::drivers::pty::get_pty(id) {
                        let pty = pty_arc.lock();
                        let has_in = if is_master {
                            !pty.master_rx.lock().is_empty()
                        } else {
                            !pty.slave_rx.lock().is_empty()
                        };
                        if has_in {
                            ready |= POLLIN;
                        }
                    }
                }
            }
            ready |= POLLOUT;
        }
        crate::fs::vfs::FileType::Epoll => {}
    }
    let interest_bits = (interest as u16 as u32) | POLLERR | POLLHUP | POLLNVAL;
    (ready & interest_bits) as i16
}

pub fn sys_ioctl(fd: u32, request: u64, arg: u64) -> SyscallResult {
    crate::fs::do_ioctl(fd, request, arg)
}

pub fn sys_ppoll(fds: u64, nfds: u32, timeout: u64, sigmask: u64) -> SyscallResult {
    if fds == 0 {
        return Err(EFAULT);
    }
    if nfds == 0 {
        return Ok(0);
    }
    if nfds > 4096 {
        return Err(EINVAL);
    }
    let _guard = SigmaskGuard { old: if sigmask != 0 {
        let mut raw = [0u8; 8];
        crate::syscall::fs::copy_from_user(sigmask, &mut raw).map_err(|_| EFAULT)?;
        Some(crate::ipc::signal::replace_current_sigmask(u64::from_le_bytes(raw)))
    } else {
        None
    }};

    let mut pollfds = alloc::vec![PollFd::default(); nfds as usize];
    let raw = unsafe {
        core::slice::from_raw_parts_mut(
            pollfds.as_mut_ptr() as *mut u8,
            pollfds.len() * core::mem::size_of::<PollFd>(),
        )
    };
    crate::syscall::fs::copy_from_user(fds, raw).map_err(|_| EFAULT)?;

    let mut immediate = false;
    let deadline = if timeout != 0 {
        let mut ts_raw = [0u8; 16];
        crate::syscall::fs::copy_from_user(timeout, &mut ts_raw).map_err(|_| EFAULT)?;
        let sec = u64::from_le_bytes(ts_raw[0..8].try_into().unwrap_or([0; 8]));
        let nsec = u64::from_le_bytes(ts_raw[8..16].try_into().unwrap_or([0; 8]));
        if nsec >= 1_000_000_000 {
            return Err(EINVAL);
        }
        if sec == 0 && nsec == 0 {
            immediate = true;
        }
        let freq = crate::arch::counter_freq().max(1);
        let ticks = sec.saturating_mul(freq).saturating_add(nsec.saturating_mul(freq) / 1_000_000_000);
        Some(crate::arch::read_counter().saturating_add(ticks))
    } else {
        None // null timeout => wait forever
    };

    loop {
        let mut ready_count = 0usize;
        for p in pollfds.iter_mut() {
            if p.fd < 0 {
                p.revents = 0;
                continue;
            }
            let mask = fd_ready_mask(p.fd, p.events);
            p.revents = mask;
            if mask != 0 {
                ready_count += 1;
            }
        }
        if ready_count > 0 {
            let raw_out = unsafe {
                core::slice::from_raw_parts(
                    pollfds.as_ptr() as *const u8,
                    pollfds.len() * core::mem::size_of::<PollFd>(),
                )
            };
            crate::syscall::fs::copy_to_user(fds, raw_out).map_err(|_| EFAULT)?;
            return Ok(ready_count);
        }
        if immediate {
            let raw_out = unsafe {
                core::slice::from_raw_parts(
                    pollfds.as_ptr() as *const u8,
                    pollfds.len() * core::mem::size_of::<PollFd>(),
                )
            };
            crate::syscall::fs::copy_to_user(fds, raw_out).map_err(|_| EFAULT)?;
            return Ok(0);
        }
        if let Some(dl) = deadline {
            if crate::arch::read_counter() >= dl {
                let raw_out = unsafe {
                    core::slice::from_raw_parts(
                        pollfds.as_ptr() as *const u8,
                        pollfds.len() * core::mem::size_of::<PollFd>(),
                    )
                };
                crate::syscall::fs::copy_to_user(fds, raw_out).map_err(|_| EFAULT)?;
                return Ok(0);
            }
        }
        crate::sched::wait::sleep_ticks(1);
        core::hint::spin_loop();
    }
}

pub fn sys_epoll_create1(flags: u32) -> SyscallResult {
    crate::fs::epoll_create1(flags)
}

pub fn sys_epoll_ctl(epfd: i32, op: i32, fd: i32, event: u64) -> SyscallResult {
    crate::fs::epoll_ctl(epfd, op, fd, event)
}

pub fn sys_epoll_pwait(epfd: i32, events: u64, maxevents: i32, timeout: i32, sigmask: u64) -> SyscallResult {
    let _guard = SigmaskGuard { old: if sigmask != 0 {
        let mut raw = [0u8; 8];
        crate::syscall::fs::copy_from_user(sigmask, &mut raw).map_err(|_| EFAULT)?;
        Some(crate::ipc::signal::replace_current_sigmask(u64::from_le_bytes(raw)))
    } else {
        None
    }};
    crate::fs::epoll_pwait(epfd, events, maxevents, timeout)
}

pub fn sys_eventfd2(initval: u32, flags: u32) -> SyscallResult {
    crate::fs::eventfd2(initval, flags)
}

pub fn sys_futex(uaddr: u64, futex_op: i32, val: u32, timeout: u64, uaddr2: u64, val3: u32) -> SyscallResult {
    crate::ipc::futex::do_futex(uaddr, futex_op, val, timeout, uaddr2, val3)
}

pub fn sys_timerfd_create(_clockid: i32, flags: i32) -> SyscallResult {
    crate::fs::timerfd_create(flags as u32)
}

pub fn sys_timerfd_settime(_fd: i32, _flags: i32, _new_value: u64, old_value: u64) -> SyscallResult {
    crate::fs::timerfd_settime(_fd, _flags, _new_value, old_value)
}

pub fn sys_signalfd4(_fd: i32, _mask: u64, _sizemask: usize, flags: i32) -> SyscallResult {
    crate::fs::signalfd4(_fd, _mask, _sizemask, flags)
}
