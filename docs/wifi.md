# Wi-Fi

Wi-Fi is under way (issue #495). `wifi` (`user/wifi`) is the daemon and its
command. It scans, joins an open or WPA2-PSK network and carries netd's
traffic over it. It drives four kinds of radio:

- the test boots' simulated radio;
- an Intel Wi-Fi 6 radio: a laptop's AX201 or an AX200 card ("Intel's
  radios");
- a MediaTek MT7601U or MT7612U USB adapter, which any arch's USB port
  takes ("MediaTek's radios");
- the Raspberry Pi's own Broadcom chip ("Broadcom's radios").

None has been tried on the hardware yet.

## `wifi`

| Command | |
|------|-|
| `wifi daemon [NIC]` | drives the radio and listens on the unix socket `wifid` (`/net/unix`, `docs/sockets-unix.md`) |
| `wifi scan` | a scan; prints the networks that answered |
| `wifi list` | what the last scan found |
| `wifi connect SSID [PASSPHRASE]` | join the network `SSID`: an open one, or a WPA2-PSK one with its passphrase |
| `wifi status` | `associated BSSID CHANNEL SSID aid N`, or `disconnected` |
| `wifi disconnect` | leave the network |

- `daemon` takes the first radio it finds: an Intel one in `/dev/pci` (when
  no `NIC` is named), else a MediaTek one in `/dev/usb`, else a Broadcom
  one in `/dev/sdio`, else the simulated one on `NIC` (default
  `/dev/net1`).
- `connect` joins the strongest network named `SSID` that the last scan
  heard, scanning first when it heard none. The passphrase is 8 to 63
  printable ASCII characters, or the PSK itself as 64 hex digits. It is a
  command argument for now, so `ps` shows it.
- `status` shows `aid 0` on a Broadcom radio, whose firmware keeps the
  association.

A network is a line `BSSID CHANNEL RSSI SECURITY SSID`, the strongest
first. `SECURITY` is `open`, `wpa2-psk`, `wpa2` (RSN with 802.1X or SAE) or
`wep`. SSID bytes outside printable ASCII are written `\xHH`.

```
02:00:5e:00:01:01 1 -40 open myos-open
02:00:5e:00:01:06 6 -55 wpa2-psk myos-wpa2
02:00:5e:00:01:0b 11 -82 wpa2-psk myos-far
```

## How the daemon works

The radio is a trait (`user/wifi/src/radio.rs`). There are three kinds:

- **A SoftMAC radio** (MediaTek's, the simulated one) is tuned to a channel
  and sends and hears 802.11 frames. The daemon does the rest.
- **A radio whose firmware scans** (Intel's) hands back the frames its scan
  heard; the daemon joins and encrypts as for a SoftMAC.
- **A FullMAC radio** (Broadcom's, `FullMac`) scans, joins and encrypts in
  its firmware and carries Ethernet frames (`daemon_fullmac` in `main.rs`).

The daemon ends on `SIGTERM`, which a power off and a reboot send to
every process (`docs/power.md`): it takes the signal through a signal fd
in its poll loop (`myos_user::signal_fd`), leaves the network, and stops
the radio (`Radio::stop`). An Intel radio's firmware is halted and the
device reset (`iwx::dev::Dev::stop`, OpenBSD's `iwx_stop_device`): a
reboot does not power an integrated radio down, and Windows' driver took
one left running for a broken device (Device Manager's warning sign, no
Wi-Fi) until the laptop was drained of power. A `SIGTERM` that comes
during a scan or a join waits for it: one longer than the power off's 2
seconds ends with `SIGKILL` and the radio running.

For the first two, the daemon does the 802.11 work itself:

- **Scanning** (`main.rs`, frames in `frame.rs`): each channel in turn, a
  wildcard probe request (none on a 5 GHz radar channel), then 120 ms of
  listening for probe responses and beacons.
- **Joining** (`mlme.rs`): open-system authentication, then association. A
  refusal names the step and the access point's status code
  (`error: association refused, status 43`).
- **802.11n**: when the radio and the access point both do it, the
  association offers HT and WMM, and data frames go as best-effort QoS
  data. The daemon declines block ack sessions, so frames come one at a
  time.
- **WPA2-PSK** (`rsn.rs` and the `wpa` crate, `user/wifi/wpa`): the 4-way
  handshake, then CCMP in software on every data frame, with replay checks
  and up to three group keys so a rekey drops nothing. The cryptography is
  RustCrypto's (`THIRD_PARTY_NOTICES.md`). `cargo test -p wpa` checks it
  against IEEE 802.11's and RFC 3394's vectors.

### The link to netd

While associated, the daemon holds a virtual NIC (`/dev/tap`, below) with
its link up and the radio's address (`tap.rs`). netd takes it as `tapN` and
asks the network's DHCP for an address. Ethernet frames go out as 802.11
data frames to the access point, with an LLC/SNAP header, and come back as
Ethernet. Leaving the network takes the NIC away, and netd lets go of it
and its lease.

## Firmware

A real radio runs its maker's firmware (the files are named in `iwx::chip`,
`mtw::chip` and `bwfm::chip`). The images never carry it: the licences
allow redistribution, but myos ships no third-party binaries.
`wifi-firmware` gets it from linux-firmware at a pinned commit:

| Command | |
|------|-|
| `wifi-firmware fetch` | downloads each missing file and keeps it only when its SHA-256 matches |
| `wifi-firmware list` | each file, `ok`, `corrupt` or `missing` |
| `wifi-firmware path` | the firmware directory |

The firmware directory is the first of `$MYOS_FIRMWARE`, `/data/firmware`
(when the data partition is mounted, `docs/install.md`; it survives a
reboot) and `/tmp/firmware`. The daemon reads only `/data/firmware`, else
`/tmp/firmware`.

A first fetch needs a network. `$WIFI_FIRMWARE_MIRROR` replaces the base
URL; when it names a directory holding a copy of linux-firmware's tree (a
USB stick), `wifi-firmware` copies from it. The tests are
`wifi_firmware_check` and `wifi_firmware_fetch` (tag `net`).

## Intel's radios

| Radio | PCI | Kind |
|------|-----|------|
| AX201 | `8086:06f0` (the ThinkPad T15p's), `8086:02f0` | integrated (CNVi): Intel x86 machines only |
| AX200 | `8086:2723` | an M.2 PCIe card, any machine with the slot |

Both are of the 22000 family and share one driver, the `iwx` crate
(`user/wifi/iwx`): a port of OpenBSD's `iwx(4)` (if_iwx.c rev 1.230) to
`no_std` Rust behind a `Device` trait. What differs between the two radios
is in `iwx::chip`. The daemon side is `user/wifi/src/intel.rs`.

The daemon reaches the radio through `/dev/pci` (`docs/pci.md`,
`user/wifi/src/pci.rs`): BAR0, DMA buffers and its MSI-X, MSI or INTx
interrupt. `/dev/pci` is there once root runs `insmod /lib/modules/pcidev`
(a line for `/data/etc/rc.local`). It is not loaded at boot because on a
machine without an IOMMU the device reaches all of memory; the laptop's
VT-d confines it to the daemon's buffers.

- `wifi scan` is the firmware's passive scan of every channel the
  regulatory domain allows.
- Joining and WPA2 are the daemon's. No key is given to the firmware: every
  frame is sent with its encryption off, as OpenBSD sends the frames it
  encrypts itself.
- The link is a non-HT station: legacy rates up to 54 Mb/s, no QoS, no
  aggregation.

`cargo test -p iwx` (CI runs it) tests the encodings and runs the driver
against a model of each radio (`user/wifi/iwx/tests/model`): bring-up with
MSI and MSI-X, a scan and a join. The model checks every command and
register access against OpenBSD's if_iwxreg.h. Whether Intel's firmware
agrees needs the hardware.

## MediaTek's radios

| Radio | What | Examples |
|-------|------|----------|
| MT7601U | 802.11b/g/n, 2.4 GHz, one antenna: the "150 Mbps" dongles | ASUS USB-N10 v2, D-Link DWA-127 rev B1, Edimax EW-7711UAn V2 |
| MT7612U | 802.11ac USB 3, 2.4 and 5 GHz, two antennas: the "AC1200" ones | ASUS USB-AC55, Netgear A6210, Alfa AWUS036ACM, the Xbox One adapter |

With one of these, a machine of any arch with a USB port can have Wi-Fi.
The MT7612U is driven as an 802.11n radio of two streams at 20 MHz; 40 and
80 MHz and 802.11ac are issue #563. The MT7610U is not supported: the only
complete driver, Linux's mt76x0u, is GPL-2.0-only, and OpenBSD's mtw(4)
does not take it.

Both are SoftMACs. The daemon (`user/wifi/src/mediatek.rs`) drives them
through `/dev/usb` (`docs/usb.md`, `user/wifi/src/usb.rs`), which is there
at boot: registers by vendor requests on `ep0`, frames on the bulk
endpoints. The driver is the `mtw` crate (`user/wifi/mtw`), behind a `Usb`
trait:

- the MT7601U: a port of OpenBSD's `mtw(4)` (if_mtw.c rev 1.13);
- the MT7612U (`mtw::mt76x2`): a port of Linux's mt76x2u.

The radio's MAC sends the daemon's frames as they are, since CCMP is the
daemon's. Rate adaptation is AMRR (`rate.rs`, from OpenBSD's
ieee80211_amrr.c), run once a second (`Radio::tick`) from the MAC's TX
statistics. Over 802.11n it uses MCS 0 to 7 on the MT7601U, and MCS 0 to 7
then 12 to 15 on the MT7612U (MCS 8 to 11 are no faster at two streams).
The MT7612U also runs its periodic calibration on that tick.

No test boot has the radio: QEMU has no USB Wi-Fi device. `cargo test -p
mtw` (CI runs it) runs the driver against a model of each chip
(`user/wifi/mtw/tests/model`): bring-up, scan, join, sending and AMRR's
climb. The models check every request and descriptor against OpenBSD's
mtwreg.h and Linux's mt76x2u.

## Broadcom's radios

The Raspberry Pi's Wi-Fi is a Broadcom (now Infineon) FullMAC chip on the
SoC's second SDHCI controller (issue #547):

| Chip | What | Boards |
|------|------|--------|
| CYW43455 | dual band 802.11ac | 3 B+, 3 A+, 4, 400, Compute Module 4 |
| CYW43438 | 2.4 GHz 802.11n | 3 B, Zero W |

The chip's firmware runs the whole 802.11 state machine: it scans, joins,
runs the WPA2 handshake with its own supplicant and encrypts. The daemon
gives it the PSK it derives from the passphrase, and exchanges Ethernet
frames with it; `rsn.rs` is not involved.

The daemon (`user/wifi/src/broadcom.rs`) drives the first card in
`/dev/sdio` (`user/wifi/src/sdio.rs`), which the `sdmmc` module serves
("SDIO" in `docs/raspberry-pi.md`); the module also raises `WL_REG_ON`. The
driver is the `bwfm` crate (`user/wifi/bwfm`), a port of OpenBSD's
`bwfm(4)` and its SDIO attachment (bwfm.c rev 1.118, if_bwfm_sdio.c rev
1.45) behind an `Sdio` trait.

Besides the firmware, the chip needs a CLM blob (regulatory data) and the
board's NVRAM file, all from `wifi-firmware fetch`. The NVRAM file is chosen
by `/proc/platform`'s `model`, the boards sharing files as linux-firmware
links them (the 3 A+ takes the 3 B+'s, the Zero W the 3 B's, the 400 and
the Compute Module 4 the 4's).

No test boot has the radio: QEMU's Raspberry Pi 3 has the SDHCI controller
but no card in it. `cargo test -p bwfm` (CI runs it) runs the driver
against a model of both chips (`user/wifi/bwfm/tests/model`) with an open
and a WPA2 network: firmware start, scan, join, frames, a wrong key.

## The simulated radio

QEMU has no Wi-Fi device, so the test boots get a simulated one: a second
virtio-net NIC (`/dev/net1`; netd keeps `/dev/net0`) on a QEMU `socket`
netdev, whose datagrams go to a thread of the launcher
(`src/wifi_sim.rs`). Each Ethernet frame carries an 802.11 frame:

| Bytes | |
|------|-|
| 0..14 | Ethernet header: broadcast, the sender's address, EtherType `0x88b5` |
| 14 | version: 1 |
| 15 | the channel it is sent on |
| 16 | the signal it arrives with, dBm (signed) |
| 17 | 0 |
| 18.. | the 802.11 frame, without its FCS |

The radio hears only the frames of the channel it is tuned to.

| SSID | BSSID | Channel | Signal | Security |
|------|-------|---------|--------|----------|
| `myos-open` | `02:00:5e:00:01:01` | 1 | −40 | open, 802.11n (HT, WMM) |
| `myos-wpa2` | `02:00:5e:00:01:06` | 6 | −55 | WPA2-PSK (CCMP), passphrase `myos-passphrase` |
| `myos-far` | `02:00:5e:00:01:0b` | 11 | −82 | WPA2-PSK (CCMP) |

The access points beacon, answer probes, authentication and association,
run the WPA2 handshake and rekey the group at a station's first data frame.
They beacon only for 5 seconds after the radio last sent a frame, so a boot
that never scans is not woken thirty times a second. Behind each one is a
router (`src/wifi_net.rs`) at 10.0.8.1/24 that leases the station
10.0.8.(1 + its association id) by DHCP and answers ARP and pings.

The tests are in `user/wifi/test.sh` (tag `wifi`): `wifi_scan`,
`wifi_connect` (open, refused and accepted WPA2 joins), and `wifi_data` and
`wifi_wpa2_data` (tags `wifi,tap`), which get netd's lease on the link and
ping 10.0.8.1 across it.

## Virtual NICs (`/dev/tap`)

The daemon's link reaches netd through a virtual NIC (`modules/tap`), as
Linux's TAP devices do: a NIC whose wire is a program. NIC `N` (0 to 3) is
there while a program holds `/dev/tap/N/prog`. Opening `prog` makes the NIC
(one holder at a time, root only); its last close takes it away.

| File | |
|------|-|
| `prog` | the program's end: a write is one Ethernet frame for netd, a read one frame netd sent (it waits; `poll` says one is there) |
| `data` | the NIC's end, as `/dev/netN/data` is |
| `ctl` | `mac ADDR`, `irq on`, `link up` or `link down` and `program N` (the count of programs that have held `prog` so far, this one included), as lines; the program writes `mac ADDR`, `up` and `down` |

- A frame netd sends while the link is down is dropped.
- Each direction queues up to 64 frames; past that the newest is dropped.
- netd takes a NIC whose link is up, with its own DHCP lease, and lets go
  of it when the link goes down or its program goes. When the `program`
  count changed between two looks, netd takes the NIC again as a new one,
  with a lease of its own (issue #649).
- The wired `net0` keeps the default route while it has a lease
  (`docs/net.md`).
- The test is `tap_smoke` (`user/c/test.sh`, tag `tap`).

## Limits

- No radio has been tried on the hardware, nor the kernel's SDIO layer.
- The passphrase is a command argument, so `ps` shows it.
- WPA3 (SAE), 802.1X, TKIP and management frame protection are not
  supported.
- Intel's radios are non-HT stations: no QoS, no aggregation.
- The MT7612U runs at 20 MHz as an 802.11n radio (issue #563).
- The MT7610U is not supported.
- The Zero 2 W's CYW43436 and the Pi 5 are not covered.
