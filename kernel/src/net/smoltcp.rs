// smoltcp TCP/IP stack integration
// Provides AF_INET TCP/UDP socket support backed by virtio-net.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use smoltcp::iface::{Config, Interface, SocketSet, SocketHandle};
use smoltcp::phy::{DeviceCapabilities, Medium};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr, Ipv4Address};

use crate::syscall::{EAGAIN, ECONNREFUSED, EINVAL};

// ─── smoltcp Device implementation ──────────────────────────────────────────

pub struct NetDevice {
    pub rx_frames: VecDeque<Vec<u8>>,
}

pub struct RxToken(Vec<u8>);
pub struct TxToken;

impl smoltcp::phy::RxToken for RxToken {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(mut self, f: F) -> R {
        f(&mut self.0)
    }
}

impl smoltcp::phy::TxToken for TxToken {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut buf = vec![0u8; len];
        let result = f(&mut buf);
        crate::drivers::virtio::net::transmit_frame(&buf);
        result
    }
}

impl smoltcp::phy::Device for NetDevice {
    type RxToken<'a> = RxToken where Self: 'a;
    type TxToken<'a> = TxToken where Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(RxToken, TxToken)> {
        let frame = self.rx_frames.pop_front()?;
        Some((RxToken(frame), TxToken))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<TxToken> {
        Some(TxToken)
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps
    }
}

// ─── Global net stack ────────────────────────────────────────────────────────

struct NetStack {
    iface: Interface,
    sockets: SocketSet<'static>,
    device: NetDevice,
}

static NET_STACK: Mutex<Option<NetStack>> = Mutex::new(None);

/// Map from kernel fd → smoltcp SocketHandle
static INET_FD_MAP: Mutex<BTreeMap<u32, SocketHandle>> = Mutex::new(BTreeMap::new());

/// Initialize the smoltcp interface. Called after virtio-net is probed.
/// `ip` is in dotted-decimal bytes, e.g. [10,0,2,15]. `gw` is the gateway.
pub fn init(mac: [u8; 6], ip: [u8; 4], prefix_len: u8) {
    let hw_addr = EthernetAddress(mac);
    let config = Config::new(hw_addr.into());
    let mut device = NetDevice { rx_frames: VecDeque::new() };

    let now = now_instant();
    let mut iface = Interface::new(config, &mut device, now);

    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(IpAddress::v4(ip[0], ip[1], ip[2], ip[3]), prefix_len));
    });

    let sockets = SocketSet::new(vec![]);

    *NET_STACK.lock() = Some(NetStack {
        iface,
        sockets,
        device,
    });
    log::info!("smoltcp interface: {}.{}.{}.{}/{}", ip[0], ip[1], ip[2], ip[3], prefix_len);
}

/// Must be called periodically (e.g. from timer_tick) to drive smoltcp.
pub fn poll() {
    // Collect received frames from virtio-net
    let frames = crate::drivers::virtio::net::take_rx_frames();

    let mut stack = NET_STACK.lock();
    let stack = match stack.as_mut() {
        Some(s) => s,
        None => return,
    };

    for frame in frames {
        stack.device.rx_frames.push_back(frame);
    }

    let now = now_instant();
    stack.iface.poll(now, &mut stack.device, &mut stack.sockets);
}

/// Create a TCP socket and bind it to an fd.
pub fn tcp_socket_create(fd: u32) {
    let mut stack = NET_STACK.lock();
    let stack = match stack.as_mut() {
        Some(s) => s,
        None => return,
    };

    let rx_buf = tcp::SocketBuffer::new(vec![0u8; 8192]);
    let tx_buf = tcp::SocketBuffer::new(vec![0u8; 8192]);
    let socket = tcp::Socket::new(rx_buf, tx_buf);
    let handle = stack.sockets.add(socket);
    INET_FD_MAP.lock().insert(fd, handle);
}

/// Connect a TCP socket to a remote address.
pub fn tcp_connect(fd: u32, addr: [u8; 4], port: u16) -> Result<(), i32> {
    let mut stack = NET_STACK.lock();
    let stack = match stack.as_mut() {
        Some(s) => s,
        None => return Err(ECONNREFUSED as i32),
    };

    let handle = match INET_FD_MAP.lock().get(&fd).copied() {
        Some(h) => h,
        None => return Err(EINVAL as i32),
    };

    let socket = stack.sockets.get_mut::<tcp::Socket>(handle);
    let remote = smoltcp::wire::IpEndpoint::new(
        IpAddress::v4(addr[0], addr[1], addr[2], addr[3]),
        port,
    );
    // Bind to any local port (use a hash of fd to avoid conflicts)
    let local_port = 49152 + (fd as u16 % 16384);
    socket.connect(stack.iface.context(), remote, local_port)
        .map_err(|_| ECONNREFUSED as i32)
}

/// Send data on a TCP socket. Returns bytes sent or error.
pub fn tcp_send(fd: u32, data: &[u8]) -> Result<usize, i32> {
    let mut stack = NET_STACK.lock();
    let stack = match stack.as_mut() {
        Some(s) => s,
        None => return Err(EAGAIN as i32),
    };

    let handle = match INET_FD_MAP.lock().get(&fd).copied() {
        Some(h) => h,
        None => return Err(EINVAL as i32),
    };

    let socket = stack.sockets.get_mut::<tcp::Socket>(handle);
    if !socket.can_send() {
        return Err(EAGAIN as i32);
    }
    socket.send_slice(data).map_err(|_| EAGAIN as i32)
}

/// Receive data from a TCP socket. Returns bytes read or error.
pub fn tcp_recv(fd: u32, buf: &mut [u8]) -> Result<usize, i32> {
    let mut stack = NET_STACK.lock();
    let stack = match stack.as_mut() {
        Some(s) => s,
        None => return Err(EAGAIN as i32),
    };

    let handle = match INET_FD_MAP.lock().get(&fd).copied() {
        Some(h) => h,
        None => return Err(EINVAL as i32),
    };

    let socket = stack.sockets.get_mut::<tcp::Socket>(handle);
    if !socket.can_recv() {
        // Check if connection closed
        if socket.state() == tcp::State::CloseWait || socket.state() == tcp::State::Closed {
            return Ok(0); // EOF
        }
        return Err(EAGAIN as i32);
    }
    socket.recv_slice(buf).map_err(|_| EAGAIN as i32)
}

/// Close and remove a TCP socket.
pub fn tcp_close(fd: u32) {
    let handle = INET_FD_MAP.lock().remove(&fd);
    if let Some(handle) = handle {
        let mut stack = NET_STACK.lock();
        if let Some(stack) = stack.as_mut() {
            stack.sockets.get_mut::<tcp::Socket>(handle).close();
            stack.sockets.remove(handle);
        }
    }
}

pub fn is_inet_fd(fd: u32) -> bool {
    INET_FD_MAP.lock().contains_key(&fd)
}

fn now_instant() -> Instant {
    let freq = crate::arch::counter_freq().max(1);
    let cnt = crate::arch::read_counter();
    Instant::from_millis((cnt * 1000 / freq) as i64)
}
