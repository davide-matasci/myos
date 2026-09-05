#![no_std]

//! Userspace smoltcp PHY over `/dev/netN`.
//!
//! Structured so a later `netd` can reuse [`Net0Device`], [`build_interface`],
//! and [`VirtualInstant`]. No `panic_handler` (this is a lib).

extern crate alloc;

use myos_user::{ioctl, open_flags, read, write_fd, O_RDWR};
use smoltcp::iface::{Config, Interface};
use smoltcp::phy::{
    Checksum, ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken,
};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress};

/// Re-export so `/ping` (and later netd) can use smoltcp 0.12 types without a
/// second copy of the crate. There is no crate-root `Dhcpv4Socket` alias in
/// 0.12; that type is `smoltcp::socket::dhcpv4::Socket`.
pub use smoltcp;

/// virtio-net DMA buffer is 2048; Ethernet payload max is `2048 - hdr`.
pub const FRAME_BUF: usize = 2048;
/// Ethernet header (14) + 1500 IP MTU.
pub const MTU: usize = 1514;

/// QEMU `virtio-net-pci` default MAC when no `mac=` is passed.
/// Used only if [`MYOS_IOCTL_NET_GETMAC`] fails.
pub const QEMU_DEFAULT_MAC: EthernetAddress =
    EthernetAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

/// Copy 6-byte MAC to the userspace pointer in `arg`.
/// Keep in sync with `myos_abi::MYOS_IOCTL_NET_GETMAC`.
pub const MYOS_IOCTL_NET_GETMAC: usize = 0x4d01;

/// Virtual clock: `u64` milliseconds bumped each poll (no clock syscall yet).
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtualInstant {
    millis: u64,
}

impl VirtualInstant {
    pub const fn new() -> Self {
        Self { millis: 0 }
    }

    pub fn now(&self) -> Instant {
        Instant::from_millis(self.millis as i64)
    }

    pub fn bump(&mut self, millis: u64) {
        self.millis = self.millis.saturating_add(millis);
    }

    pub fn millis(&self) -> u64 {
        self.millis
    }
}

/// smoltcp `Device` over an open `/dev/net0` fd (raw Ethernet frames).
pub struct Net0Device {
    fd: usize,
    rx: [u8; FRAME_BUF],
    rx_len: usize,
}

impl Net0Device {
    /// Open `/dev/net0` read/write. Returns `None` if the chrdev is missing.
    pub fn open() -> Option<Self> {
        Self::open_path(b"/dev/net0")
    }

    /// Open an arbitrary net chrdev path (e.g. `/dev/net1`).
    pub fn open_path(path: &[u8]) -> Option<Self> {
        let fd = open_flags(path, O_RDWR)?;
        Some(Self {
            fd,
            rx: [0; FRAME_BUF],
            rx_len: 0,
        })
    }

    /// Open `/dev/netN` for `n` in 0..=9.
    pub fn open_nth(n: usize) -> Option<Self> {
        if n > 9 {
            return None;
        }
        let mut path = *b"/dev/net0";
        path[8] = b'0' + (n as u8);
        Self::open_path(&path)
    }

    pub fn fd(&self) -> usize {
        self.fd
    }

    /// Hardware MAC via ioctl; falls back to [`QEMU_DEFAULT_MAC`] on failure.
    pub fn mac(&self) -> EthernetAddress {
        let mut mac = [0u8; 6];
        if ioctl(self.fd, MYOS_IOCTL_NET_GETMAC, mac.as_mut_ptr() as usize) != usize::MAX {
            EthernetAddress(mac)
        } else {
            QEMU_DEFAULT_MAC
        }
    }
}

/// Build an Ethernet `Interface` with the device MAC (ioctl), software checksums,
/// and the given timestamp.
pub fn build_interface(device: &mut Net0Device, now: Instant) -> Interface {
    let mac = device.mac();
    let mut config = Config::new(HardwareAddress::Ethernet(mac));
    // Stable seed derived from MAC; not cryptographic.
    let mut seed = 0u64;
    for (i, b) in mac.0.iter().enumerate() {
        seed |= (*b as u64) << (8 * i);
    }
    config.random_seed = seed;
    Interface::new(config, device, now)
}

pub fn device_capabilities() -> DeviceCapabilities {
    let mut caps = DeviceCapabilities::default();
    caps.medium = Medium::Ethernet;
    caps.max_transmission_unit = MTU;
    caps.max_burst_size = Some(1);
    // Driver did not negotiate virtio checksum offload. In smoltcp 0.12,
    // `Checksum::Both` (default) means the *stack* verifies RX and computes TX
    // in software. `Checksum::None` / `ignored()` skips checksums entirely.
    // Struct is `#[non_exhaustive]`; assign fields on Default.
    let mut csum = ChecksumCapabilities::default();
    csum.ipv4 = Checksum::Both;
    csum.udp = Checksum::Both;
    csum.tcp = Checksum::Both;
    csum.icmpv4 = Checksum::Both;
    caps.checksum = csum;
    caps
}

pub struct Net0RxToken<'a> {
    buf: &'a [u8],
}

pub struct Net0TxToken {
    fd: usize,
}

impl Device for Net0Device {
    type RxToken<'a> = Net0RxToken<'a>;
    type TxToken<'a> = Net0TxToken;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let n = read(self.fd, &mut self.rx);
        // 0 = no frame this poll (busy-poll). usize::MAX = error.
        if n == 0 || n == usize::MAX {
            return None;
        }
        self.rx_len = n.min(FRAME_BUF);
        Some((
            Net0RxToken {
                buf: &self.rx[..self.rx_len],
            },
            Net0TxToken { fd: self.fd },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(Net0TxToken { fd: self.fd })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        device_capabilities()
    }
}

impl RxToken for Net0RxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(self.buf)
    }
}

impl TxToken for Net0TxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = [0u8; FRAME_BUF];
        let n = len.min(FRAME_BUF);
        let r = f(&mut buf[..n]);
        if n != 0 {
            let _ = write_fd(self.fd, &buf[..n]);
        }
        r
    }
}
