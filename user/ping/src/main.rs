#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use myos_net::smoltcp::iface::SocketSet;
use myos_net::smoltcp::phy::Device;
use myos_net::smoltcp::socket::{dhcpv4, icmp};
use myos_net::smoltcp::wire::{
    Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr,
};
use myos_net::{build_interface, Net0Device, VirtualInstant};
use myos_user::{exit, exit_code, heap_init, Heap, write};

#[global_allocator]
static GLOBAL: Heap = Heap;

const GATEWAY: IpAddress = IpAddress::v4(10, 0, 2, 2);
const ICMP_IDENT: u16 = 0x22b;
const DHCP_POLLS: usize = 3000;
const ICMP_POLLS: usize = 2000;
const TICK_MS: u64 = 100;

#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    main()
}

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_argc: usize, _argv: *const usize) -> ! {
    main()
}

fn fail(msg: &[u8]) -> ! {
    write(msg);
    exit_code(1);
}

fn main() -> ! {
    heap_init();

    let Some(mut device) = Net0Device::open() else {
        fail(b"open /dev/net0 fail\n");
    };

    let mut clock = VirtualInstant::new();
    let mut iface = build_interface(&mut device, clock.now());

    let mut sockets = SocketSet::new(Vec::new());
    let dhcp_handle = sockets.add(dhcpv4::Socket::new());

    let mut dhcp_ok = false;
    for _ in 0..DHCP_POLLS {
        clock.bump(TICK_MS);
        let now = clock.now();
        iface.poll(now, &mut device, &mut sockets);

        let (addr, router) = match sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).poll() {
            Some(dhcpv4::Event::Configured(cfg)) => (Some(cfg.address), cfg.router),
            Some(dhcpv4::Event::Deconfigured) => {
                iface.update_ip_addrs(|addrs| addrs.clear());
                iface.routes_mut().remove_default_ipv4_route();
                (None, None)
            }
            None => (None, None),
        };
        if let Some(addr) = addr {
            iface.update_ip_addrs(|addrs| {
                addrs.clear();
                let _ = addrs.push(IpCidr::Ipv4(addr));
            });
            if let Some(router) = router {
                let _ = iface.routes_mut().add_default_ipv4_route(router);
            } else {
                iface.routes_mut().remove_default_ipv4_route();
            }
            dhcp_ok = true;
            break;
        }
    }
    if !dhcp_ok {
        fail(b"dhcp fail\n");
    }

    let icmp_rx = icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY], vec![0; 256]);
    let icmp_tx = icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY], vec![0; 256]);
    let icmp_handle = sockets.add(icmp::Socket::new(icmp_rx, icmp_tx));

    let mut sent = false;
    for _ in 0..ICMP_POLLS {
        clock.bump(TICK_MS);
        let now = clock.now();
        iface.poll(now, &mut device, &mut sockets);

        let checksum = device.capabilities().checksum;
        let socket = sockets.get_mut::<icmp::Socket>(icmp_handle);
        if !socket.is_open() {
            let _ = socket.bind(icmp::Endpoint::Ident(ICMP_IDENT));
        }
        if !sent && socket.can_send() {
            let echo = Icmpv4Repr::EchoRequest {
                ident: ICMP_IDENT,
                seq_no: 1,
                data: b"ping",
            };
            if let Ok(payload) = socket.send(echo.buffer_len(), GATEWAY) {
                let mut pkt = Icmpv4Packet::new_unchecked(payload);
                echo.emit(&mut pkt, &checksum);
                sent = true;
            }
        }
        if socket.can_recv() {
            if let Ok((payload, _)) = socket.recv() {
                if let Ok(pkt) = Icmpv4Packet::new_checked(payload) {
                    if let Ok(Icmpv4Repr::EchoReply { ident, seq_no, .. }) =
                        Icmpv4Repr::parse(&pkt, &checksum)
                    {
                        if ident == ICMP_IDENT && seq_no == 1 {
                            write(b"ping ok\n");
                            exit();
                        }
                    }
                }
            }
        }
    }
    fail(b"icmp fail\n");
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    myos_user::panic_die(info);
}
