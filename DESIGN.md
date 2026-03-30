# Cyllor OS - Rust Operating System Design

## Context
从零开发一个 Rust 操作系统，支持 AArch64/x86-64 双架构，多核调度，最终目标运行 XFCE 桌面。
核心原则：**最少代码，最大复用**。

## Architecture Overview

```
┌─────────────────────────────────────────────────┐
│                 XFCE Desktop                     │
│        (GTK + libX11/Wayland client)             │
├─────────────────────────────────────────────────┤
│              Display Server (Wayland)            │
├─────────────────────────────────────────────────┤
│           Linux Syscall ABI Layer                │
│     (运行未修改的 Linux ELF 二进制)               │
├──────────────┬──────────────────────────────────┤
│   VFS / ext4 │  TCP/IP Stack  │  IPC / Signals  │
├──────────────┼──────────────────────────────────┤
│         Process & Thread Manager                 │
│         SMP Scheduler (per-CPU runqueue)         │
├──────────────┴──────────────────────────────────┤
│           Memory Manager (VMM + PMM)             │
│        (generic 4-level page table abstraction)  │
├─────────────────────────────────────────────────┤
│              HAL (Hardware Abstraction)           │
│     ┌──────────────┬──────────────────┐          │
│     │   x86_64     │    AArch64       │          │
│     │ APIC/IOAPIC  │    GIC           │          │
│     │ PCI/PCIe     │    PCI/PCIe      │          │
│     │ HPET/PIT     │    Generic Timer │          │
│     └──────────────┴──────────────────┘          │
├─────────────────────────────────────────────────┤
│         UEFI Boot (Limine Protocol)              │
└─────────────────────────────────────────────────┘
```

## Project Structure (Cargo Workspace)

```
cyllor/
├── Cargo.toml                    # workspace root
├── Makefile                      # build/run shortcuts
├── limine.conf                   # bootloader config
│
├── kernel/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs               # entry point, dispatcher
│       │
│       ├── arch/                  # 架构相关 (最小化)
│       │   ├── mod.rs             # trait定义 + cfg dispatch
│       │   ├── x86_64/
│       │   │   ├── mod.rs
│       │   │   ├── boot.rs        # Limine entry
│       │   │   ├── gdt.rs         # GDT/TSS
│       │   │   ├── idt.rs         # 中断描述符表
│       │   │   ├── apic.rs        # Local APIC + IOAPIC
│       │   │   ├── paging.rs      # x86-64 page table操作
│       │   │   ├── context.rs     # 上下文切换 (汇编)
│       │   │   └── syscall.rs     # SYSCALL/SYSRET entry
│       │   └── aarch64/
│       │       ├── mod.rs
│       │       ├── boot.rs
│       │       ├── exceptions.rs  # 异常向量表
│       │       ├── gic.rs         # GICv3 中断控制器
│       │       ├── paging.rs      # AArch64 page table
│       │       ├── context.rs     # 上下文切换
│       │       └── syscall.rs     # SVC entry
│       │
│       ├── mm/                    # 内存管理 (通用)
│       │   ├── mod.rs
│       │   ├── pmm.rs             # 物理内存分配 (bitmap/buddy)
│       │   ├── vmm.rs             # 虚拟地址空间管理
│       │   ├── heap.rs            # 内核堆 (linked_list_allocator)
│       │   ├── page_table.rs      # 通用 PageTable trait
│       │   └── mmap.rs            # mmap/munmap 实现
│       │
│       ├── sched/                 # 调度器 (通用)
│       │   ├── mod.rs
│       │   ├── process.rs         # Process / Thread 结构
│       │   ├── scheduler.rs       # CFS-like 多核调度器
│       │   ├── cpu.rs             # per-CPU 数据
│       │   └── wait.rs            # 等待队列 / futex
│       │
│       ├── syscall/               # Linux ABI 兼容层
│       │   ├── mod.rs             # syscall dispatcher
│       │   ├── fs.rs              # open/read/write/close/stat...
│       │   ├── process.rs         # fork/exec/exit/wait/clone...
│       │   ├── mm.rs              # mmap/brk/mprotect...
│       │   ├── net.rs             # socket/bind/listen/accept...
│       │   ├── signal.rs          # kill/sigaction/sigprocmask...
│       │   ├── time.rs            # clock_gettime/nanosleep...
│       │   └── io.rs              # ioctl/poll/epoll/select...
│       │
│       ├── fs/                    # 文件系统
│       │   ├── mod.rs             # VFS 层
│       │   ├── vfs.rs             # inode/dentry/superblock
│       │   ├── ext4.rs            # ext4 (用 ext4-rs crate)
│       │   ├── tmpfs.rs           # tmpfs
│       │   ├── devfs.rs           # /dev
│       │   ├── procfs.rs          # /proc
│       │   └── pipe.rs            # pipe/fifo
│       │
│       ├── net/                   # 网络栈
│       │   ├── mod.rs
│       │   └── smoltcp.rs         # 基于 smoltcp crate
│       │
│       ├── drivers/               # 设备驱动
│       │   ├── mod.rs
│       │   ├── uart.rs            # 串口 (调试)
│       │   ├── pci.rs             # PCI 枚举
│       │   ├── virtio/            # VirtIO 驱动族
│       │   │   ├── mod.rs
│       │   │   ├── block.rs       # virtio-blk
│       │   │   ├── net.rs         # virtio-net
│       │   │   ├── gpu.rs         # virtio-gpu
│       │   │   └── input.rs       # virtio-input
│       │   ├── framebuffer.rs     # GOP/线性帧缓冲
│       │   └── nvme.rs            # NVMe (可选)
│       │
│       ├── ipc/                   # 进程间通信
│       │   ├── mod.rs
│       │   ├── signal.rs          # POSIX 信号
│       │   └── futex.rs           # futex
│       │
│       └── sync/                  # 同步原语
│           ├── mod.rs
│           ├── spinlock.rs
│           ├── mutex.rs
│           └── rwlock.rs
│
└── tools/
    ├── mkimage.sh                 # 制作磁盘镜像
    └── run-qemu.sh               # QEMU 启动脚本
```

## Key Design Decisions

### 1. Bootloader: Limine
- 同时支持 x86-64 和 AArch64
- 提供统一的 Limine Boot Protocol（内存映射、帧缓冲、ACPI 表）
- 使用 `limine` Rust crate 解析引导信息
- **零自研引导代码**

### 2. Kernel: Monolithic (宏内核)
- 比微内核代码量少得多（无需 IPC 消息传递开销）
- 驱动在内核态运行，减少上下文切换
- Linux 本身就是宏内核，ABI 兼容更自然

### 3. HAL 代码复用策略
```rust
// arch/mod.rs - 统一 trait 定义
pub trait Arch {
    fn init_interrupts();
    fn enable_interrupts();
    fn disable_interrupts();
    fn halt();
    fn context_switch(old: &mut Context, new: &Context);
    fn set_page_table(root: PhysAddr);
    fn current_cpu_id() -> usize;
    fn send_ipi(cpu: usize, vector: u8);
}

#[cfg(target_arch = "x86_64")]
pub use x86_64::X86_64Arch as PlatformArch;
#[cfg(target_arch = "aarch64")]
pub use aarch64::AArch64Arch as PlatformArch;
```
- **90%+ 代码是架构无关的**，通过 trait + cfg 切换
- 架构相关代码限制在 `arch/` 目录，每个架构约 1000-1500 行

### 4. Linux Syscall 兼容
- 实现 ~100 个核心 syscall 即可运行 XFCE
- 关键 syscalls: `read/write/open/close/mmap/fork/execve/clone/wait4/ioctl/poll/epoll/socket/...`
- x86-64: 通过 `SYSCALL` 指令进入，`RAX` = syscall number
- AArch64: 通过 `SVC #0` 进入，`X8` = syscall number
- ELF loader 加载未修改的 Linux 动态链接库

### 5. SMP 多核调度
```
Per-CPU Architecture:
┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐
│  CPU 0  │  │  CPU 1  │  │  CPU 2  │  │  CPU 3  │
│ runqueue│  │ runqueue│  │ runqueue│  │ runqueue│
│ idle    │  │ idle    │  │ idle    │  │ idle    │
└────┬────┘  └────┬────┘  └────┬────┘  └────┬────┘
     │            │            │            │
     └────────────┴─────┬──────┴────────────┘
                        │
                   Work Stealing
                   Load Balancing
```
- CFS-like 调度器（红黑树按 vruntime 排序）
- Per-CPU 运行队列，减少锁竞争
- Work-stealing 负载均衡
- 支持 `clone()` with `CLONE_THREAD` for pthreads

### 6. Graphics Stack (运行 XFCE)
```
XFCE → GTK → libwayland-client → Wayland compositor (userspace)
                                        ↓
                                   DRM/KMS ioctl
                                        ↓
                                  Kernel framebuffer / virtio-gpu
```
- 内核提供 DRM/KMS 接口 (通过 ioctl)
- Userspace 运行 Wayland compositor (如 wlroots-based)
- XFCE 4.18+ 已原生支持 Wayland

## Key Rust Crates (最大化复用)

| Crate | 用途 | 替代自研代码量 |
|-------|------|---------------|
| `limine` | 引导协议 | ~2000 行 |
| `x86_64` | x86 寄存器/页表/GDT/IDT | ~3000 行 |
| `aarch64-cpu` | AArch64 寄存器操作 | ~1500 行 |
| `linked_list_allocator` | 内核堆分配器 | ~500 行 |
| `smoltcp` | TCP/IP 协议栈 | ~10000 行 |
| `acpi` + `aml` | ACPI 表解析 | ~5000 行 |
| `log` + `spinning_top` | 日志 + 自旋锁 | ~300 行 |
| `bitflags` | 标志位操作 | ~200 行 |
| `goblin` | ELF 解析 | ~1500 行 |

**预估总节省: ~24000 行代码**

## Implementation Phases

### Phase 1: 可启动内核 (Boot + 串口输出)
- Limine 引导 → 内核入口
- 串口 UART 输出 (调试)
- GDT/IDT (x86) 或异常向量 (AArch64)
- 物理内存管理 (PMM)
- 虚拟内存 + 内核堆
- **验证**: QEMU 启动，串口打印 "Cyllor OS booted on {arch}"

### Phase 2: 多核 + 中断
- APIC/GIC 初始化
- AP 核心启动 (SMP bringup)
- 定时器中断 (抢占式调度基础)
- **验证**: 多核打印各自 CPU ID

### Phase 3: 进程调度
- Process/Thread 结构
- CFS 调度器 + per-CPU runqueue
- 上下文切换
- 内核线程创建
- **验证**: 多个内核线程在多核上并发运行

### Phase 4: 用户态 + Syscall
- Ring 3 / EL0 切换
- Syscall 入口 (SYSCALL/SVC)
- ELF loader
- 基本 syscalls: exit, write, mmap, brk
- **验证**: 运行静态链接的 "Hello World" ELF

### Phase 5: VFS + 文件系统
- VFS 层 (inode, dentry, file)
- tmpfs, devfs, procfs
- ext4 (读取)
- open/read/write/close/stat
- **验证**: 用户程序读写文件

### Phase 6: 进程管理完善
- fork/exec/wait
- Signal 机制
- Pipe
- 动态链接器支持 (ld-linux)
- **验证**: 运行 busybox sh

### Phase 7: 网络 + 设备驱动
- VirtIO block/net
- smoltcp 网络栈
- Socket syscalls
- **验证**: 用户程序进行 TCP 连接

### Phase 8: 图形 + 桌面
- DRM/KMS ioctl 接口
- Framebuffer / virtio-gpu
- Wayland compositor (移植)
- XFCE 桌面环境
- **验证**: XFCE 桌面启动运行

## Build & Run

```bash
# x86-64
make run ARCH=x86_64

# AArch64
make run ARCH=aarch64

# 内部调用:
# cargo build --target x86_64-unknown-none
# 或 cargo build --target aarch64-unknown-none
# + limine 生成 ISO
# + qemu-system-{arch} 启动
```

## Verification Strategy
- **主验证平台: AArch64** (qemu-system-aarch64)
- 每个阶段优先在 AArch64 上验证通过，再移植验证 x86-64
- 串口输出作为主要调试手段
- Phase 4 后可运行 Linux 测试二进制
- Phase 6 后可运行 busybox 测试套件
