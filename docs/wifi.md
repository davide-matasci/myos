# Wi-Fi

Wi-Fi is under way (issue #495): `wifi` is the daemon and its command
(`user/wifi`). Through the test boots' simulated radio it scans, joins
an open network and carries netd's traffic over it; on a laptop with
Intel's AX201 it brings the radio up and scans (below, "The AX201").
WPA2, and joining through the AX201, come next.

## `wifi`

| Command | |
|------|-|
| `wifi daemon [NIC]` | drives the radio: the AX201 when `/dev/pci` has one and no `NIC` is named, else the simulated one on `NIC` (default `/dev/net1`); listens on the unix socket `wifid` (`/net/unix`, `docs/sockets-unix.md`) |
| `wifi scan` | a scan (the simulated radio's 2.4 GHz channels, every channel the AX201's regulatory domain allows); prints the networks that answered |
| `wifi list` | what the last scan found |
| `wifi connect SSID` | join the network `SSID`, the strongest the last scan heard (after a scan when it heard none); open networks only for now |
| `wifi status` | `associated BSSID CHANNEL SSID aid N`, or `disconnected` |
| `wifi disconnect` | leave the network |

A command is one line over one connection to `wifid`; the daemon answers
and ends the connection. A network is a line `BSSID CHANNEL RSSI SECURITY
SSID`, the strongest first:

```
02:00:5e:00:01:01 1 -40 open myos-open
02:00:5e:00:01:06 6 -55 wpa2-psk myos-wpa2
02:00:5e:00:01:0b 11 -82 wpa2-psk myos-far
```

`SECURITY` is `open`, `wpa2-psk` (RSN with pre-shared keys), `wpa2`
(RSN with another key management: 802.1X, SAE) or `wep` (privacy
without RSN). Bytes of an SSID outside printable ASCII are written
`\xHH`.

A scan tunes the radio to each channel in turn, sends a wildcard probe
request and listens 120 ms for probe responses and beacons (every
102.4 ms), keeping each BSSID once with its strongest signal
(`user/wifi/src/main.rs`; the frames: `frame.rs`). The radio is a trait
(`radio.rs`): tuned to a channel, it sends and hears 802.11 frames, and
says its address; a radio whose firmware scans by itself (the AX201)
hands back the frames its scan heard instead.

Joining (`mlme.rs`) is open-system authentication, then association: a
request and its answer each, sent again (three times, 300 ms apart) when
no answer comes. A refusal names the step and the status code the access
point gave (`error: association refused, status 43`). `connect` first
leaves the network it is on (a deauthentication), and an access point's
deauthentication or disassociation ends the link.

While associated, the daemon holds a virtual NIC (`/dev/tap`, below; its
address the radio's) with its link up, so netd takes it as `tapN` and
asks the network's DHCP for an address (`tap.rs`). An Ethernet frame
netd sends on it goes out as a data frame to the access point (to the
distribution system: the BSSID, the station, the destination; an
LLC/SNAP header before the EtherType), and a data frame from the access
point to the station or a group comes back in as Ethernet (`frame.rs`).
Leaving the network, or the access point letting the station go, takes
the NIC away, and netd lets go of it and its lease. A scan while
associated tunes the radio back to the link's channel when it is done.

## Firmware

A real radio runs Intel's firmware (the AX201's is
`iwlwifi-QuZ-a0-hr-b0-77.ucode`). The images never carry it: its licence
lets it be redistributed unmodified, but myos ships no third-party
binaries. `wifi-firmware` gets it:

| Command | |
|------|-|
| `wifi-firmware fetch` | downloads each file from linux-firmware at a pinned commit, checks its SHA-256 (a file that does not match is not kept) and keeps it in the firmware directory; one already there is left alone |
| `wifi-firmware list` | each file, `ok`, `corrupt` or `missing` |
| `wifi-firmware path` | the firmware directory |

The directory is `$MYOS_FIRMWARE`, else `/data/firmware` when the boot
disk's data partition is mounted at `/data` (`docs/install.md`; it is
there after a reboot), else `/tmp/firmware`. A first fetch needs a
network: wired, or `$WIFI_FIRMWARE_MIRROR` naming a directory with a copy
of linux-firmware's tree (a USB stick) to copy from instead. The tests are
`wifi_firmware_check` (a file with the wrong SHA-256 is refused) and
`wifi_firmware_fetch` (tag `net`: the real download). The daemon reads
the firmware from `/data/firmware`, else `/tmp/firmware` (it does not
see `$MYOS_FIRMWARE`).

## The AX201

Intel's AX201 (PCI `8086:06f0`, the ThinkPad T15p's, and `8086:02f0`:
the ones with a QuZ MAC) is driven by the daemon itself, through
`/dev/pci` (`docs/pci-acpi-smp.md`; `user/wifi/src/pci.rs`): BAR0 mapped,
DMA buffers, its MSI-X or MSI interrupt. The driver is the `iwx` crate
(`user/wifi/iwx`), a port of OpenBSD's `iwx(4)` (if_iwx.c rev 1.230) to
`no_std` Rust behind a `Device` trait, so it builds and its encodings are
tested on the host (`cargo test -p iwx`):

| Module | |
|------|-|
| `fw.rs` | the firmware file: its TLVs, the runtime image's LMAC, UMAC and paging sections, API and capability bits, command versions |
| `reg.rs`, `hw.rs` | registers; the NIC lock, the periphery window, preparing, resetting and starting the hardware, interrupt masks, the MAC address |
| `ring.rs` | the context information the firmware loads itself from, the command queue's descriptors and commands, the packets of a receive buffer |
| `msg.rs` | the commands up to a scan and the answers and notifications read (ALIVE, the NVM, the regulatory domain, the scan, a received frame) |
| `dev.rs` | the bring-up: memory shared with the firmware, its load up to ALIVE, the interrupt (ICT or MSI-X causes), commands and their answers, the configuration |

Bring-up is one pass, as the 22000 family's unified firmware wants: the
hardware prepared and started, the firmware's sections copied into DMA
memory and named in the context information, the self load started and
ALIVE waited for (1 s); then the NVM access commands, the NVM itself (the
SKU's bands, the antennas, the channels), the antennas, Bluetooth
coexistence off, the SoC's crystal latency, the device kept awake, the
regulatory domain asked of the firmware's location-aware regulatory
(world `ZZ` to start; its answer replaces the NVM's channels), and the
scan configuration. The DMA memory is nine buffers of `/dev/pci`'s
sixteen: control (context information, ICT, receive descriptors and
status), the command queue's descriptors, its commands, the 512 receive
buffers in four, and the firmware's two images (the paging one stays the
device's while it runs).

`wifi scan` on the AX201 is the firmware's passive scan of every allowed
channel (110 ms each), every beacon and probe response passed up; the
daemon reads the networks from them as from the simulated radio's. The
radio does not send yet, so `wifi connect` answers `error: this radio
scans only so far`: transmit, association and the data path through the
AX201 are what is left of issue #495. No test boot has the device: the
driver's encodings are what CI tests, the rest needs the laptop.

## The simulated radio

QEMU has no Wi-Fi device, so the test boots get a simulated one: a second
virtio-net NIC (`/dev/net1`; netd keeps `/dev/net0`) on a QEMU `socket`
netdev whose UDP datagrams go to a thread of the launcher
(`src/wifi_sim.rs`). Each Ethernet frame carries an 802.11 frame, without
its FCS:

| Bytes | |
|------|-|
| 0..14 | Ethernet header: broadcast destination, the sender's address, EtherType `0x88b5` (IEEE 802 local experimental) |
| 14 | version: 1 |
| 15 | the channel it is sent on |
| 16 | the signal it arrives with, dBm (signed; the access points set it) |
| 17 | 0 |
| 18.. | the 802.11 frame |

The radio hears only the frames of the channel it is tuned to. The
launcher's access points beacon on their channels every 100 TU and answer
the probe requests on them (for their SSID or the wildcard), take
open-system authentication and association (a WPA2 one refuses an
association request without an RSN element: status 43), give each
association the next association id, and let a station go at its
deauthentication. An associated station's data frames reach a router
behind each one (`src/wifi_net.rs`): 10.0.8.1/24, the access point's
address, leasing the station 10.0.8.(1 + its association id) by DHCP,
answering ARP for 10.0.8.1 and the pings sent to it. They beacon
only for 5 seconds after the radio last sent a frame: a boot that never
scans is not woken thirty times a second for nothing.

| SSID | BSSID | Channel | Signal | Security |
|------|-------|---------|--------|----------|
| `myos-open` | `02:00:5e:00:01:01` | 1 | −40 | open |
| `myos-wpa2` | `02:00:5e:00:01:06` | 6 | −55 | WPA2-PSK (CCMP) |
| `myos-far` | `02:00:5e:00:01:0b` | 11 | −82 | WPA2-PSK (CCMP) |

The tests (`user/wifi/test.sh`, tag `wifi`) start the daemon. `wifi_scan`
scans and expects those three lines, from `scan` and `list` alike.
`wifi_connect` joins `myos-open` (`status` names it, aid 1), is refused
`myos-wpa2` (not supported yet, the open network left meanwhile), joins
again (aid 2) and disconnects. `wifi_data` (tags `wifi,tap`) joins
`myos-open`, waits for netd's lease on the link's NIC, pings 10.0.8.1
across it, checks that `net0` keeps the default route, and that the NIC
goes at `disconnect`.

## Virtual NICs (`/dev/tap`)

The link the daemon brings up reaches netd through a virtual NIC
(`modules/tap`), as Linux's TAP devices do: a NIC whose wire is a program.
NIC `N` (0 to 3) is there while a program holds `/dev/tap/N/prog`
(opening it makes the NIC, one holder at a time, root only; its last
close takes the NIC away):

| File | |
|------|-|
| `prog` | the program's end: a write is one Ethernet frame for netd, a read one frame netd sent (it waits for one; `poll` says one is there) |
| `data` | the NIC's end, as `/dev/netN/data` is: a write sends a frame, a read takes one (0 bytes when none waits), `poll` says one is there |
| `ctl` | `mac ADDR`, `irq on` and `link up` or `link down`, as lines; the program writes `mac ADDR`, `up` and `down` (the association) |

A frame netd sends while the link is down is dropped. Each direction
queues up to 64 frames; past that the newest is dropped, as a full NIC
drops. The test is `tap_smoke` (`user/c/test.sh`, tag `tap`).

netd takes a NIC whose link is up, with its own DHCP lease, and lets go
of it when its link goes down or its program goes; the wired `net0`
keeps the default route while it has a lease (`docs/sockets-curl.md`,
`multihome_smoke`).
