pub mod fs;
mod process;
mod mm;
mod net;
mod signal;
mod time;
mod io;

use crate::arch::TrapFrame;

// Linux syscall numbers (AArch64 numbering — must be updated per arch)
mod nr {
    pub const READ: u64 = 63;
    pub const WRITE: u64 = 64;
    pub const CLOSE: u64 = 57;
    pub const OPENAT: u64 = 56;
    pub const FSTAT: u64 = 80;
    pub const EXIT: u64 = 93;
    pub const EXIT_GROUP: u64 = 94;
    pub const SET_TID_ADDRESS: u64 = 96;
    pub const CLOCK_GETTIME: u64 = 113;
    pub const NANOSLEEP: u64 = 101;
    pub const MMAP: u64 = 222;
    pub const MUNMAP: u64 = 215;
    pub const MPROTECT: u64 = 226;
    pub const BRK: u64 = 214;
    pub const CLONE: u64 = 220;
    pub const EXECVE: u64 = 221;
    pub const WAIT4: u64 = 260;
    pub const GETPID: u64 = 172;
    pub const GETUID: u64 = 174;
    pub const GETGID: u64 = 176;
    pub const GETEUID: u64 = 175;
    pub const GETEGID: u64 = 177;
    pub const GETTID: u64 = 178;
    pub const IOCTL: u64 = 29;
    pub const WRITEV: u64 = 66;
    pub const READV: u64 = 65;
    pub const PPOLL: u64 = 73;
    pub const FCNTL: u64 = 25;
    pub const DUP: u64 = 23;
    pub const DUP3: u64 = 24;
    pub const PIPE2: u64 = 59;
    pub const SOCKET: u64 = 198;
    pub const BIND: u64 = 200;
    pub const LISTEN: u64 = 201;
    pub const ACCEPT4: u64 = 242;
    pub const CONNECT: u64 = 203;
    pub const SENDTO: u64 = 206;
    pub const RECVFROM: u64 = 207;
    pub const SETSOCKOPT: u64 = 208;
    pub const GETSOCKOPT: u64 = 209;
    pub const GETCWD: u64 = 17;
    pub const CHDIR: u64 = 49;
    pub const MKDIRAT: u64 = 34;
    pub const UNLINKAT: u64 = 35;
    pub const RENAMEAT2: u64 = 276;
    pub const STATX: u64 = 291;
    pub const NEWFSTATAT: u64 = 79;
    pub const LSEEK: u64 = 62;
    pub const RT_SIGACTION: u64 = 134;
    pub const RT_SIGPROCMASK: u64 = 135;
    pub const RT_SIGRETURN: u64 = 139;
    pub const KILL: u64 = 129;
    pub const TGKILL: u64 = 131;
    pub const FUTEX: u64 = 98;
    pub const EPOLL_CREATE1: u64 = 20;
    pub const EPOLL_CTL: u64 = 21;
    pub const EPOLL_PWAIT: u64 = 22;
    pub const EVENTFD2: u64 = 19;
    pub const TIMERFD_CREATE: u64 = 85;
    pub const TIMERFD_SETTIME: u64 = 86;
    pub const SIGNALFD4: u64 = 74;
    pub const GETRANDOM: u64 = 278;
    pub const MEMFD_CREATE: u64 = 279;
    pub const PRLIMIT64: u64 = 261;
    pub const SCHED_YIELD: u64 = 124;
    pub const CLOCK_NANOSLEEP: u64 = 115;
    pub const SET_ROBUST_LIST: u64 = 99;
    pub const GET_ROBUST_LIST: u64 = 100;
    // Additional glibc-required syscalls
    pub const RSEQ: u64 = 293;
    pub const CLONE3: u64 = 435;
    pub const PRCTL: u64 = 167;
    pub const ARCH_PRCTL: u64 = 167;
    pub const UNAME: u64 = 160;
    pub const GETDENTS64: u64 = 61;
    pub const READLINKAT: u64 = 78;
    pub const ACCESS: u64 = 1039; // faccessat
    pub const FACCESSAT: u64 = 48;
    pub const FACCESSAT2: u64 = 439;
    pub const FSTATFS: u64 = 44;
    pub const STATFS: u64 = 43;
    pub const MREMAP: u64 = 216;
    pub const MADVISE: u64 = 233;
    pub const SIGALTSTACK: u64 = 132;
    pub const GETSOCKNAME: u64 = 204;
    pub const GETPEERNAME: u64 = 205;
    pub const SHMGET: u64 = 194;
    pub const SHMAT: u64 = 196;
    pub const SHMCTL: u64 = 195;
    pub const SHMDT: u64 = 197;
}

/// Handle a syscall from userspace
pub fn handle(frame: &mut TrapFrame) {
    let syscall_nr = frame.regs[8]; // x8 = syscall number
    let args = [
        frame.regs[0], // x0
        frame.regs[1], // x1
        frame.regs[2], // x2
        frame.regs[3], // x3
        frame.regs[4], // x4
        frame.regs[5], // x5
    ];

    // Trace syscalls
    static SC: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let sc_num = SC.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // Minimal syscall trace with ret
    crate::drivers::uart::write_byte(b'[');
    crate::drivers::uart::write_byte(b'0' + ((syscall_nr / 100) % 10) as u8);
    crate::drivers::uart::write_byte(b'0' + ((syscall_nr / 10) % 10) as u8);
    crate::drivers::uart::write_byte(b'0' + (syscall_nr % 10) as u8);

    let result = match syscall_nr {
        nr::WRITE => fs::sys_write(args[0], args[1], args[2]),
        nr::READ => fs::sys_read(args[0], args[1], args[2]),
        nr::OPENAT => fs::sys_openat(args[0] as i32, args[1], args[2] as u32, args[3] as u32),
        nr::CLOSE => fs::sys_close(args[0] as u32),
        nr::LSEEK => fs::sys_lseek(args[0] as u32, args[1] as i64, args[2] as u32),
        nr::FSTAT => fs::sys_fstat(args[0] as u32, args[1]),
        nr::NEWFSTATAT => fs::sys_newfstatat(args[0] as i32, args[1], args[2], args[3] as u32),
        nr::GETCWD => fs::sys_getcwd(args[0], args[1]),
        nr::CHDIR => fs::sys_chdir(args[0]),
        nr::MKDIRAT => fs::sys_mkdirat(args[0] as i32, args[1], args[2] as u32),
        nr::UNLINKAT => fs::sys_unlinkat(args[0] as i32, args[1], args[2] as u32),
        nr::WRITEV => fs::sys_writev(args[0] as u32, args[1], args[2] as u32),
        nr::READV => fs::sys_readv(args[0] as u32, args[1], args[2] as u32),
        nr::DUP => fs::sys_dup(args[0] as u32),
        nr::DUP3 => fs::sys_dup3(args[0] as u32, args[1] as u32, args[2] as u32),
        nr::PIPE2 => fs::sys_pipe2(args[0], args[1] as u32),
        nr::FCNTL => fs::sys_fcntl(args[0] as u32, args[1] as u32, args[2]),

        nr::EXIT => process::sys_exit(args[0] as i32),
        nr::EXIT_GROUP => process::sys_exit_group(args[0] as i32),
        nr::GETPID => process::sys_getpid(),
        nr::GETTID => process::sys_gettid(),
        nr::GETUID | nr::GETEUID => Ok(0), // root
        nr::GETGID | nr::GETEGID => Ok(0), // root
        nr::CLONE => process::sys_clone(args[0], args[1], args[2], args[3], args[4]),
        nr::EXECVE => process::sys_execve(args[0], args[1], args[2]),
        nr::WAIT4 => process::sys_wait4(args[0] as i32, args[1], args[2] as u32, args[3]),
        nr::SET_TID_ADDRESS => process::sys_set_tid_address(args[0]),
        nr::SCHED_YIELD => { crate::sched::timer_tick(); Ok(0) }

        nr::MMAP => mm::sys_mmap(args[0], args[1], args[2] as u32, args[3] as u32, args[4] as i32, args[5] as i64),
        nr::MUNMAP => mm::sys_munmap(args[0], args[1]),
        nr::MPROTECT => mm::sys_mprotect(args[0], args[1], args[2] as u32),
        nr::BRK => mm::sys_brk(args[0]),

        nr::RT_SIGACTION => signal::sys_rt_sigaction(args[0] as i32, args[1], args[2], args[3]),
        nr::RT_SIGPROCMASK => signal::sys_rt_sigprocmask(args[0] as i32, args[1], args[2], args[3]),
        nr::RT_SIGRETURN => signal::sys_rt_sigreturn(frame),
        nr::KILL => signal::sys_kill(args[0] as i32, args[1] as i32),
        nr::TGKILL => signal::sys_tgkill(args[0] as i32, args[1] as i32, args[2] as i32),

        nr::CLOCK_GETTIME => time::sys_clock_gettime(args[0] as u32, args[1]),
        nr::NANOSLEEP => time::sys_nanosleep(args[0], args[1]),
        nr::CLOCK_NANOSLEEP => time::sys_clock_nanosleep(args[0] as u32, args[1] as u32, args[2], args[3]),

        nr::IOCTL => io::sys_ioctl(args[0] as u32, args[1], args[2]),
        nr::PPOLL => io::sys_ppoll(args[0], args[1] as u32, args[2], args[3]),
        nr::EPOLL_CREATE1 => io::sys_epoll_create1(args[0] as u32),
        nr::EPOLL_CTL => io::sys_epoll_ctl(args[0] as i32, args[1] as i32, args[2] as i32, args[3]),
        nr::EPOLL_PWAIT => io::sys_epoll_pwait(args[0] as i32, args[1], args[2] as i32, args[3] as i32, args[4]),
        nr::EVENTFD2 => io::sys_eventfd2(args[0] as u32, args[1] as u32),
        nr::FUTEX => io::sys_futex(args[0], args[1] as i32, args[2] as u32, args[3], args[4], args[5] as u32),

        nr::SOCKET => net::sys_socket(args[0] as i32, args[1] as i32, args[2] as i32),
        nr::BIND => net::sys_bind(args[0] as i32, args[1], args[2] as u32),
        nr::LISTEN => net::sys_listen(args[0] as i32, args[1] as i32),
        nr::ACCEPT4 => net::sys_accept4(args[0] as i32, args[1], args[2], args[3] as u32),
        nr::CONNECT => net::sys_connect(args[0] as i32, args[1], args[2] as u32),
        nr::SENDTO => net::sys_sendto(args[0] as i32, args[1], args[2], args[3] as u32, args[4], args[5] as u32),
        nr::RECVFROM => net::sys_recvfrom(args[0] as i32, args[1], args[2], args[3] as u32, args[4], args[5]),

        nr::GETRANDOM => mm::sys_getrandom(args[0], args[1], args[2] as u32),
        nr::PRLIMIT64 => process::sys_prlimit64(args[0] as i32, args[1] as u32, args[2], args[3]),
        nr::SET_ROBUST_LIST => Ok(0),
        nr::GET_ROBUST_LIST => Err(ENOSYS),
        nr::MEMFD_CREATE => mm::sys_memfd_create(args[0], args[1] as u32),

        // glibc required stubs
        nr::RSEQ => Err(ENOSYS), // glibc handles ENOSYS gracefully
        nr::CLONE3 => Err(ENOSYS), // falls back to clone
        nr::PRCTL => process::sys_prctl(args[0] as i32, args[1], args[2], args[3], args[4]),
        nr::UNAME => process::sys_uname(args[0]),
        nr::GETDENTS64 => fs::sys_getdents64(args[0] as u32, args[1], args[2] as u32),
        nr::READLINKAT => fs::sys_readlinkat(args[0] as i32, args[1], args[2], args[3] as u32),
        nr::FACCESSAT | nr::FACCESSAT2 => Ok(0), // pretend everything is accessible
        nr::FSTATFS | nr::STATFS => process::sys_statfs(args[0], args[1]),
        nr::MREMAP => mm::sys_mremap(args[0], args[1], args[2], args[3] as u32, args[4]),
        nr::MADVISE => Ok(0), // advisory, ignore
        nr::SIGALTSTACK => Ok(0), // stub
        nr::GETSOCKNAME => Ok(0),
        nr::GETPEERNAME => Err(ENOTSOCK),

        _ => {
            log::warn!("Unimplemented syscall: {syscall_nr}");
            Err(ENOSYS)
        }
    };

    // Return value in x0
    let ret = match result {
        Ok(val) => val as u64,
        Err(errno) => (-errno as i64) as u64,
    };
    frame.regs[0] = ret;
    crate::drivers::uart::write_byte(b']');
}

pub type SyscallResult = Result<usize, i32>;

// Error codes
pub const EPERM: i32 = 1;
pub const ENOENT: i32 = 2;
pub const ESRCH: i32 = 3;
pub const EINTR: i32 = 4;
pub const EIO: i32 = 5;
pub const ENXIO: i32 = 6;
pub const EBADF: i32 = 9;
pub const ECHILD: i32 = 10;
pub const EAGAIN: i32 = 11;
pub const ENOMEM: i32 = 12;
pub const EACCES: i32 = 13;
pub const EFAULT: i32 = 14;
pub const EEXIST: i32 = 17;
pub const ENOTDIR: i32 = 20;
pub const EISDIR: i32 = 21;
pub const EINVAL: i32 = 22;
pub const EMFILE: i32 = 24;
pub const ENFILE: i32 = 23;
pub const ENOSPC: i32 = 28;
pub const EPIPE: i32 = 32;
pub const ERANGE: i32 = 34;
pub const ENOSYS: i32 = 38;
pub const ENOTEMPTY: i32 = 39;
pub const ENOTSOCK: i32 = 88;
pub const EOPNOTSUPP: i32 = 95;
pub const EAFNOSUPPORT: i32 = 97;
pub const ECONNREFUSED: i32 = 111;
