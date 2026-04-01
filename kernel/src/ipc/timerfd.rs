use alloc::sync::Arc;
use spin::Mutex;
use crate::syscall::{SyscallResult, EINVAL, EBADF};
use crate::fs::vfs::{FileObject, FileType, SpecialData};
use alloc::collections::BTreeMap;

pub struct TimerFdState {
    pub interval_ticks: u64, // 0 = one-shot (stored as ticks)
    pub next_expiry: u64,    // absolute tick count
    pub count: u64,          // expiry count (read atomically)
    pub armed: bool,
}

impl TimerFdState {
    pub fn new() -> Self {
        Self { interval_ticks: 0, next_expiry: 0, count: 0, armed: false }
    }
}

static TIMERFDS: Mutex<BTreeMap<u32, TimerFdState>> = Mutex::new(BTreeMap::new());

pub fn create(clockid: i32, flags: i32) -> SyscallResult {
    let file = Arc::new(Mutex::new(FileObject {
        inode: None, offset: 0, flags: flags as u32,
        ftype: FileType::TimerFd,
        special_data: Some(SpecialData::TimerFd(0)), // placeholder, updated after alloc
    }));
    let fd = crate::fs::alloc_fd(file.clone())? as u32;
    // Update with real fd
    file.lock().special_data = Some(SpecialData::TimerFd(fd));
    TIMERFDS.lock().insert(fd, TimerFdState::new());
    Ok(fd as usize)
}

pub fn settime(fd: u32, flags: i32, new_value: u64, old_value: u64) -> SyscallResult {
    // struct itimerspec: { it_interval: timespec, it_value: timespec }
    // timespec: { tv_sec: i64, tv_nsec: i64 }
    // new_value: ptr to itimerspec
    if new_value == 0 { return Err(EINVAL); }

    let it_interval_sec  = unsafe { *(new_value as *const i64) };
    let it_interval_nsec = unsafe { *((new_value + 8) as *const i64) };
    let it_value_sec     = unsafe { *((new_value + 16) as *const i64) };
    let it_value_nsec    = unsafe { *((new_value + 24) as *const i64) };

    let freq = crate::arch::counter_freq();
    let now = crate::arch::read_counter();

    let interval_ns = it_interval_sec as u64 * 1_000_000_000 + it_interval_nsec as u64;
    let value_ns = it_value_sec as u64 * 1_000_000_000 + it_value_nsec as u64;

    // Convert ns to ticks
    let value_ticks = if freq > 0 { (value_ns * freq) / 1_000_000_000 } else { value_ns };
    let interval_ticks = if freq > 0 { (interval_ns * freq) / 1_000_000_000 } else { interval_ns };

    let mut timerfds = TIMERFDS.lock();
    let state = timerfds.entry(fd).or_insert_with(TimerFdState::new);

    if old_value != 0 {
        // Write current timer value (zeroed)
        unsafe { core::ptr::write_bytes(old_value as *mut u8, 0, 32); }
    }

    state.count = 0;
    if value_ns == 0 {
        state.armed = false;
    } else {
        state.next_expiry = now + value_ticks;
        state.interval_ticks = interval_ticks;
        state.armed = true;
    }

    Ok(0)
}

pub fn gettime(fd: u32, curr_value: u64) -> SyscallResult {
    if curr_value == 0 { return Ok(0); }
    let freq = crate::arch::counter_freq();
    let now = crate::arch::read_counter();

    let timerfds = TIMERFDS.lock();
    if let Some(state) = timerfds.get(&fd) {
        // it_interval
        let interval_ns = if state.interval_ticks > 0 && freq > 0 {
            (state.interval_ticks * 1_000_000_000) / freq
        } else { 0 };
        // it_value: time remaining
        let remaining_ns = if state.armed && state.next_expiry > now && freq > 0 {
            ((state.next_expiry - now) * 1_000_000_000) / freq
        } else { 0 };
        unsafe {
            *((curr_value + 0) as *mut i64) = (interval_ns / 1_000_000_000) as i64;
            *((curr_value + 8) as *mut i64) = (interval_ns % 1_000_000_000) as i64;
            *((curr_value + 16) as *mut i64) = (remaining_ns / 1_000_000_000) as i64;
            *((curr_value + 24) as *mut i64) = (remaining_ns % 1_000_000_000) as i64;
        }
    } else {
        unsafe { core::ptr::write_bytes(curr_value as *mut u8, 0, 32); }
    }
    Ok(0)
}

/// Called from timer tick to advance all timers
pub fn tick_all() {
    let now = crate::arch::read_counter();
    let mut fds = TIMERFDS.lock();
    for state in fds.values_mut() {
        if !state.armed { continue; }
        if now >= state.next_expiry {
            state.count += 1;
            if state.interval_ticks > 0 {
                state.next_expiry += state.interval_ticks;
            } else {
                state.armed = false;
            }
        }
    }
}

pub fn read_count(fd: u32, buf: u64, count: usize) -> SyscallResult {
    if count < 8 { return Err(EINVAL); }
    let mut fds = TIMERFDS.lock();
    if let Some(state) = fds.get_mut(&fd) {
        if state.count == 0 {
            return Err(crate::syscall::EAGAIN); // Would block
        }
        let c = state.count;
        state.count = 0;
        unsafe { *(buf as *mut u64) = c; }
        Ok(8)
    } else {
        Err(EBADF)
    }
}

pub fn is_readable(fd: u32) -> bool {
    TIMERFDS.lock().get(&fd).map(|s| s.count > 0).unwrap_or(false)
}
