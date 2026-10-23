# Wi-Fi

Wi-Fi is under way (issue #495): `wifi` is the daemon and its command
(`user/wifi`). It scans, joins an open or WPA2-PSK network and carries
netd's traffic over it, through the test boots' simulated radio, an
Intel Wi-Fi 6 radio (a laptop's AX201 or an AX200 card: below, "Intel's
radios") or a MediaTek MT7601U USB adapter, which any arch's USB port
takes ("MediaTek's radios"); none yet tried on the hardware.

## `wifi`

| Command | |
|------|-|
| `wifi daemon [NIC]` | drives the radio: an AX201 or AX200 when `/dev/pci` has one and no `NIC` is named, else an MT7601U when `/dev/usb` has one, else the simulated one on `NIC` (default `/dev/net1`); listens on the unix socket `wifid` (`/net/unix`, `docs/sockets-unix.md`) |
| `wifi scan` | a scan (the 2.4 GHz channels of the simulated radio and the MT7601U, every channel an Intel radio's regulatory domain allows); prints the networks that answered |
| `wifi list` | what the last scan found |
| `wifi connect SSID [PASSPHRASE]` | join the network `SSID`, the strongest the last scan heard (after a scan when it heard none): an open one, or a WPA2-PSK one with its passphrase (8 to 63 printable ASCII characters, or the PSK itself as 64 hex digits) |
| `wifi status` | `associated BSSID CHANNEL SSID aid N`, or `disconnected` |
| `wifi disconnect` | leave the network |

A command is one line over one connection to `wifid` (`connect`'s
passphrase on a second line); the daemon answers and ends the
connection. The passphrase is an argument for now, so `ps` shows it. A network is a line `BSSID CHANNEL RSSI SECURITY
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
says its address; a radio whose firmware scans by itself (Intel's)
hands back the frames its scan heard instead, and a SoftMAC radio
(MediaTek's) is tuned and listened to like the simulated one.

Joining (`mlme.rs`) is open-system authentication, then association: a
request and its answer each, sent again (three times, 300 ms apart) when
no answer comes. A refusal names the step and the status code the access
point gave (`error: association refused, status 43`). `connect` first
leaves the network it is on (a deauthentication), and an access point's
deauthentication or disassociation ends the link.

The station offers the 802.11b and g rates (1 to 54 Mb/s, the faster
four as Extended Supported Rates). With a radio that does 802.11n
itself (`Radio::ht`, its HT Capabilities: the MT7601U's, and the
simulated radio's) and an 802.11n access point (its beacon's HT
Capabilities and WMM elements), the association request also offers
802.11n: the radio's HT Capabilities and a WMM Information element, as
an 802.11n station is a QoS one. The link is then a QoS link: the
daemon sends QoS data frames of best effort (EAPOL frames go as plain
data), and declines a block ack session the access point offers (its
ADDBA request answered with status 37), so frames come one at a time.
The radio learns which it is (`Radio::associated`), and gets a tick a
second while associated (`Radio::tick`: the MT7601U's rate adaptation).

On a WPA2-PSK network (`rsn.rs`, the `wpa` crate in `user/wifi/wpa`)
the association request carries an RSN element (CCMP, PSK), and the
4-way handshake follows (IEEE 802.11-2020 12.7.6): the pairwise master
key is PBKDF2-HMAC-SHA1 of the passphrase and SSID; message 1 brings the
access point's nonce and the station answers with its own (from
`/dev/urandom`) and its RSN element, under a MIC made with the pairwise
key both now derive; message 3 proves the access point has that key and
brings the group key (AES key wrap), its RSN element checked against the
beacon's; message 4 ends it, and the keys are in. Three seconds cover the
whole handshake. A wrong passphrase shows as the access point letting
the station go after message 2 (`error: 4-way handshake: the access
point refused the key: wrong passphrase?`) or as message 3's MIC being
wrong. From then on every data frame is CCMP's (AES-CCM, an 8-byte MIC;
12.5.3): sent with the pairwise key, received with it or with a group key
by the index the frame names (up to three are kept, so a rekey does not
drop the frames still under the old one), a frame whose packet number
is not past the last one's dropped as a replay. A group key handshake
(the access point rekeying the group) is answered while the link is up.
The cryptography is RustCrypto's (`THIRD_PARTY_NOTICES.md`); `cargo test
-p wpa` checks the keys, the EAPOL-Key frames and CCMP against IEEE
802.11's PSK vectors, RFC 3394's key wrap and vectors another
implementation made (Python's `hmac` and `cryptography`). WPA3 (SAE), 802.1X, TKIP and management
frame protection are not supported.

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

A real radio runs its maker's firmware (the AX201's is
`iwlwifi-QuZ-a0-hr-b0-77.ucode`, the AX200's `iwlwifi-cc-a0-77.ucode`:
`iwx::chip` names each; the MT7601U's MCU runs `mt7601u.bin`,
`mtw::chip`'s). The images never carry it: Intel's and MediaTek's
licences let it be redistributed unmodified, but myos ships no
third-party binaries. `wifi-firmware` gets it:

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

## Intel's radios

Two of Intel's Wi-Fi 6 radios, of the same (22000) family: one driver,
the same rings, commands and firmware protocol, each radio's own facts
in `iwx::chip`:

| Radio | PCI | MAC | | Firmware |
|------|-----|-----|-|----------|
| AX201 | `8086:06f0` (Comet Lake H, the ThinkPad T15p's), `8086:02f0` | QuZ | integrated (CNVi: the MAC in Intel's chipset, so Intel x86 machines only); crystal latency 500 µs | `iwlwifi-QuZ-a0-hr-b0-77.ucode` |
| AX200 | `8086:2723` | CC | discrete: an M.2 PCIe card, for any machine with the slot | `iwlwifi-cc-a0-77.ucode` |

What differs, as OpenBSD's iwx_attach sets it up: the MAC the PCI id
must have (HW_REV), whether the hardware start forces power gating and
resets again (integrated only), where the latency tolerance for the
firmware's boot goes (the periphery's `HPM_UMAC_LTR`, a discrete radio's
`LTR_LONG_VAL_AD`), and SOC_CONFIGURATION (the `DISCRETE` flag, the
crystal latency). The two firmware files agree in everything the driver
checks (command versions, API and capability bits, limits).

The daemon drives the radio itself (`user/wifi/src/intel.rs`), the
first `/dev/pci` has whose id is in the table, through `/dev/pci`
(`docs/pci-acpi-smp.md`; `user/wifi/src/pci.rs`): BAR0 mapped, DMA
buffers, its MSI-X or MSI interrupt, or its INTx line (all the aarch64
and riscv64 boards give a PCI function, an AX200 card's on them:
serviced as MSI's, then unmasked again through `/dev/pci`'s `irq`).
`/dev/pci` is there once root loaded its module, `insmod /lib/modules/pcidev` (a line for
`/data/etc/rc.local`): it is not loaded at boot, since on a board without
an IOMMU the daemon's device reaches all of memory; the laptop's VT-d
confines it to the daemon's buffers. The driver is the `iwx` crate
(`user/wifi/iwx`), a port of OpenBSD's `iwx(4)` (if_iwx.c rev 1.230) to
`no_std` Rust behind a `Device` trait, so it builds and its encodings are
tested on the host (`cargo test -p iwx`):

| Module | |
|------|-|
| `chip.rs` | the radios: their ids, MAC, integrated or discrete, crystal latency, firmware file |
| `fw.rs` | the firmware file: its TLVs, the runtime image's LMAC, UMAC and paging sections, API and capability bits, command versions |
| `reg.rs`, `hw.rs` | registers; the NIC lock, the periphery window, preparing, resetting and starting the hardware, interrupt masks, the MAC address |
| `ring.rs` | the context information the firmware loads itself from, the command queue's descriptors and commands, the packets of a receive buffer |
| `msg.rs` | the commands up to a scan and the answers and notifications read (ALIVE, the NVM, the regulatory domain, the scan, a received frame) |
| `sta.rs` | joining as a station: the PHY, MAC and binding contexts, the station and its transmit queue, the session protection, the TX command and its response, the beacon timing the firmware needs |
| `dev.rs` | the bring-up: memory shared with the firmware, its load up to ALIVE, the interrupt (ICT or MSI-X causes), commands and their answers, the configuration; joining, sending, leaving |

Bring-up is one pass, as the 22000 family's unified firmware wants: the
hardware prepared and started, the firmware's sections copied into DMA
memory and named in the context information, the self load started and
ALIVE waited for (1 s); then the NVM access commands, the NVM itself (the
SKU's bands, the antennas, the channels), the antennas, Bluetooth
coexistence off, the SoC's crystal latency, the device kept awake, the
regulatory domain asked of the firmware's location-aware regulatory
(world `ZZ` to start; its answer replaces the NVM's channels), and the
scan configuration. The DMA memory is eleven buffers of `/dev/pci`'s
sixteen: control (context information, ICT, receive descriptors and
status), the command queue's descriptors, its commands, the 512 receive
buffers in four, the firmware's two images (the paging one stays the
device's while it runs), and at the first join the station's transmit
queue: its 256 descriptors with the byte count table, and 64 slots of
4 KB for the frames on their way.

`wifi scan` on an Intel radio is the firmware's passive scan of every allowed
channel (110 ms each), every beacon and probe response passed up; the
daemon reads the networks from them as from the simulated radio's, and
keeps each access point's beacon timing (interval, DTIM, timestamp) and
rates for the firmware.

Joining is the daemon's, as with the simulated radio (`mlme.rs`,
`rsn.rs`), and the radio is told each step (`Radio::joining`,
`associated`, `left`). Before authentication the firmware gets the
network (iwx_auth): a PHY context on its channel (20 MHz) and its
receive chains, a MAC context (a BSS station of the BSSID, beacons and
group frames up), their binding, the access point as station 0, its
transmit queue (SCD_QUEUE_CONFIG, queue 1, the management TID), and a
session protection that keeps it on the channel for the authentication
and association (at least 2000 TU). The daemon's frames then go out on
that queue, each a TX command with its 802.11 header (padded to 4 bytes)
and the body in a transfer buffer of its own: management and group
frames at the lowest rate (1 Mb/s, 6 on 5 GHz), and EAPOL ahead of the
rest; its response frees the slot and says whether the frame was
acknowledged. Once associated (iwx_run): the station and the MAC
context updated (associated, its id, the DTIM timing from the beacon),
the smart FIFO on, every group frame passed up, the device kept awake,
and the firmware's rate scaling over the access point's non-HT rates.
Leaving (iwx_run_stop, iwx_deauth) drains and flushes what was queued,
then removes the queue, the station, the binding and the contexts.

It is a non-HT station: legacy rates up to 54 Mb/s, no QoS (the
association request asks for none, so the access point sends plain data
frames), no aggregation. On a WPA2 network the daemon encrypts and
decrypts as it does over the simulated radio (CCMP in software): no key
is given to the firmware, every frame is sent with its encryption off
(TX_FLAGS_ENCRYPT_DIS, as OpenBSD sends the frames it encrypts itself),
and the protected frames it receives come up as they were sent; the two
bytes of padding the firmware puts after a frame's header (after its
CCMP header when it reports the frame as CCMP's) are taken out.

No test boot has either radio. `cargo test -p iwx` (CI runs it) tests
the driver's encodings, and runs it against a model of each
(`user/wifi/iwx/tests/model`, `radios.rs`; what differs between them in
its `Hardware`): the registers and periphery, the
interrupt (MSI with the ICT, MSI-X with its automask), the receive ring
and transmit queues in the driver's DMA buffers, and a firmware that
loads itself from the context information (checking the sections are
those of `iwx::sim`, a synthetic file in Intel's format with myos's own
bytes), sends ALIVE and answers every command of the bring-up, the scan
and joining a network as the real one does; three access points (the
simulated radio's, below) beacon and answer authentication and
association. Every command, descriptor and register access is checked
against the firmware's structures as OpenBSD's if_iwxreg.h describes
them, and the first thing a real device would not take stops it with
the reason, the radio's own facts included (its MAC, where it takes the
latency tolerance, its SoC configuration). The tests bring each radio
up with MSI and with MSI-X, scan (hearing the three networks at their
signals), join `myos-open`, authenticate, associate and leave, and an
AX200 the driver takes for integrated is refused. What the model cannot
show is whether Intel's firmware agrees with that reading of it: the
rest needs the hardware.

## MediaTek's radios

The MT7601U: an 802.11b/g/n USB adapter for 2.4 GHz with one antenna
(the cheap "150 Mbps" dongles: ASUS USB-N10 v2, D-Link DWA-127 rev B1,
Edimax EW-7711UAn V2, Xiaomi's and many unbranded ones), so a machine of
any arch with a USB port can have Wi-Fi: a Raspberry Pi, a RISC-V board.
Its ids and firmware are in `mtw::chip`, a table the MT7610U and MT7612U
can join (they share the MAC; their RF and 5 GHz setup would be new:
issue #553).

It is a SoftMAC: its MCU calibrates the radio and takes the frames, the
host does the rest, so the daemon scans (tuning to each channel and
sending probe requests, as for the simulated radio), joins and encrypts,
and the radio is told the network it joins (`Radio::joining`,
`associated`, `left`). The daemon drives it (`user/wifi/src/mediatek.rs`)
through `/dev/usb` (`docs/usb.md`; `user/wifi/src/usb.rs`): the first
device there whose id is in the table, its `ctl` held, its registers
read and written with vendor requests on `ep0` (a 32-bit register is two
`WRITE_2`s), the frames heard read from its first bulk IN endpoint (a
read is one transfer, which the MAC fills with up to 2 KiB of frames),
the MCU's commands, the firmware and the management frames written to
its first bulk OUT endpoint and the data frames to the next one (best
effort; one per access category). `/dev/usb` is there at boot. The
driver is the `mtw` crate (`user/wifi/mtw`), a port of OpenBSD's
`mtw(4)` (if_mtw.c rev 1.13) to `no_std` Rust behind a `Usb` trait:

| Module | |
|------|-|
| `chip.rs` | the radios: their USB ids, ASIC, firmware file and where it goes |
| `fw.rs` | the firmware file: its header, interrupt vectors, instruction and data parts |
| `reg.rs` | registers, vendor requests, the eFUSE ROM's words, the descriptors' flags, the MAC, baseband and RF defaults, the channels' RF settings, the legacy rates |
| `usb.rs` | the `Usb` trait (vendor requests, bulk OUT) and which bulk endpoint is which |
| `dev.rs` | the bring-up: the WLAN core powered, the firmware loaded and started, the eFUSE ROM read (address, calibration), the MAC, baseband and RF set up and calibrated; channels, joining, sending, leaving, stopping |
| `rx.rs`, `tx.rs` | the frames as the endpoints carry them: an aggregated transfer's frames after their RXWI (signal, flags; a padded header put back), a frame to send after its TXD and TXWI (rate, ACK, the station's entry) |

Bring-up (mtw_attachhook, mtw_init): the WLAN core and its oscillator
on, the firmware written in 8 KiB blocks to the MCU's memory, its
interrupt vectors last (a firmware already running, a daemon started
again without the device reset, is kept), the eFUSE ROM read (the
address, the frequency offset, each channel's transmit power, the rate
groups' power, the LNA gain and signal offset), the MAC reset and given
its defaults and address, the key tables cleared, the radio on, the RF
banks and the baseband given their defaults, the RF calibrated by the
MCU (frequency offset, temperature, VCO, RX DC), channel 1, then DMA,
receive aggregation and a station's receive filter (frames to another
station, control frames, duplicates and damaged ones dropped). A
channel is its four RF registers, its transmit power, the LNA and a VCO
calibration.

The daemon's frames go out as they are, the MAC putting nothing in
them (`TXD_WIV`: the IV is the frame's, CCMP being the daemon's): a
management or group frame at 1 Mb/s, a data frame to the access point
as the access point's entry in the station table, which association
sets, at the rate AMRR chose (`rate.rs`, OpenBSD's ieee80211_amrr.c).
The rates are those both ends have, slowest first: MCS 0 to 7 (one
stream, 20 MHz, a long guard interval: 6.5 to 65 Mb/s) on an 802.11n
link, the access point's legacy rates on another. AMRR starts at the
slowest and, once a second (`Radio::tick`), reads the MAC's TX
statistics (cleared when read: successes, retries, failures): a second
of more than 10 frames with under a tenth retried goes up a rate, one
with over a third retried goes down, and an increase that fails right
away makes the next wait twice as long (up to 10 seconds); between, the
hardware falls back on its own on a frame's retries (OFDM down to 6
Mb/s, CCK to 1). A frame wanting an ACK gets the ACK's duration at its
control rate. The frames heard come up as the access point sent them
(no key is in the MAC), with their signal in dBm from the ROM's offsets.
Once associated: the slot time and CCK preamble the access point's
capabilities allow, its basic rates, the BSSID; the beacon timer (TSF
sync) stays off, since the station never sleeps.

No test boot has the radio: QEMU has no USB Wi-Fi device. `cargo test
-p mtw` (CI runs it) tests the encodings (the firmware header, the
endpoints, aggregated receive transfers, the transmit descriptors), and
runs the driver against a model of the MT7601U (`user/wifi/mtw/tests/model`,
`radio.rs`): its registers behind the vendor requests, the eFUSE ROM,
the RF and baseband windows (a temperature reading and the RX DC
calibration finishing as the real ones do), the MCU taking a synthetic
firmware file in blocks and starting on its vectors only when its code
is where it belongs, its commands, and the bulk endpoints: frames out to
the air on the channel the RF registers and VCO calibration tuned to,
and back in aggregated as the MAC does, past its receive filter; the
air is the Intel model's (the three access points). Every request and
descriptor is checked against OpenBSD's mtwreg.h, and the first a real
device would not take stops it with the reason (a frame sent with the
MAC's transmit off, a data frame whose station entry is not its access
point, the firmware written before the MCU's DMA is set up, a PHY mode
or MCS the chip has not). The model's air carries a frame only as fast
as the access point's signal allows (all of 802.11n at −40 dBm, 9 Mb/s
at −82): one sent faster is retried 7 times and fails, as the TX
statistics then count. The tests bring the radio up (the calibrations
in order), keep a running firmware, refuse another chip, scan the three
channels (each network at its signal), join `myos-open`, send to it and
leave; over 802.11n there AMRR climbs from MCS 0 to MCS 7 in seven
seconds and stays, and on `myos-far` (802.11g, −82 dBm) it climbs to 9
Mb/s and tries 11 now and then, more rarely each time. `/dev/usb` itself
is tested in the boots (`usbdev`, with a USB serial adapter: `docs/usb.md`).

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
deauthentication. A WPA2 one, passphrase `myos-passphrase`, is the
authenticator of the 4-way handshake (a deauthentication, reason 15,
when message 2's MIC is wrong), encrypts and decrypts the data frames
with CCMP, and starts a group key handshake (a new group key) at a
station's first data frame after the handshake, as an access point
rekeying does; its next group frames still go out under the old key
until the station confirms. An associated station's data frames reach a router
behind each one (`src/wifi_net.rs`): 10.0.8.1/24, the access point's
address, leasing the station 10.0.8.(1 + its association id) by DHCP,
answering ARP for 10.0.8.1 and the pings sent to it. They beacon
only for 5 seconds after the radio last sent a frame: a boot that never
scans is not woken thirty times a second for nothing.

| SSID | BSSID | Channel | Signal | Security |
|------|-------|---------|--------|----------|
| `myos-open` | `02:00:5e:00:01:01` | 1 | −40 | open, 802.11n (HT, WMM) |
| `myos-wpa2` | `02:00:5e:00:01:06` | 6 | −55 | WPA2-PSK (CCMP) |
| `myos-far` | `02:00:5e:00:01:0b` | 11 | −82 | WPA2-PSK (CCMP) |

The tests (`user/wifi/test.sh`, tag `wifi`) start the daemon. `wifi_scan`
scans and expects those three lines, from `scan` and `list` alike.
`wifi_connect` joins `myos-open` (`status` names it, aid 1), is refused
`myos-wpa2` without a passphrase (the open network left meanwhile) and
with a wrong one (aid 2, let go), joins `myos-open` again (aid 3), then
`myos-wpa2` with its passphrase (aid 4), and disconnects. `wifi_data`
(tags `wifi,tap`) joins `myos-open` over 802.11n (the simulated radio
offers HT, and the access point drops a plain data frame from an
802.11n station: the link's are QoS data), waits for netd's lease on the
link's NIC, pings 10.0.8.1 across it, checks that `net0` keeps the
default route, and that the NIC goes at `disconnect`; `wifi_wpa2_data`
does the same over `myos-wpa2`, so the lease comes under both group
keys and the ping under the pairwise one.

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
