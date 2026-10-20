# Wi-Fi

Wi-Fi is under way (issue #495): `wifi` is the daemon and its command
(`user/wifi`). It scans, joins an open or WPA2-PSK network and carries
netd's traffic over it, through the test boots' simulated radio or a
laptop's Intel AX201 (below, "The AX201"; not yet tried on the hardware).

## `wifi`

| Command | |
|------|-|
| `wifi daemon [NIC]` | drives the radio: the AX201 when `/dev/pci` has one and no `NIC` is named, else the simulated one on `NIC` (default `/dev/net1`); listens on the unix socket `wifid` (`/net/unix`, `docs/sockets-unix.md`) |
| `wifi scan` | a scan (the simulated radio's 2.4 GHz channels, every channel the AX201's regulatory domain allows); prints the networks that answered |
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
says its address; a radio whose firmware scans by itself (the AX201)
hands back the frames its scan heard instead.

Joining (`mlme.rs`) is open-system authentication, then association: a
request and its answer each, sent again (three times, 300 ms apart) when
no answer comes. A refusal names the step and the status code the access
point gave (`error: association refused, status 43`). `connect` first
leaves the network it is on (a deauthentication), and an access point's
deauthentication or disassociation ends the link.

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
DMA buffers, its MSI-X or MSI interrupt, or its INTx line (all the
aarch64 and riscv64 boards give a PCI function: serviced as MSI's, then
unmasked again through `/dev/pci`'s `irq`). `/dev/pci` is there once root
loaded its module, `insmod /lib/modules/pcidev` (a line for
`/data/etc/rc.local`): it is not loaded at boot, since on a board without
an IOMMU the daemon's device reaches all of memory; the laptop's VT-d
confines it to the daemon's buffers. The driver is the `iwx` crate
(`user/wifi/iwx`), a port of OpenBSD's `iwx(4)` (if_iwx.c rev 1.230) to
`no_std` Rust behind a `Device` trait, so it builds and its encodings are
tested on the host (`cargo test -p iwx`):

| Module | |
|------|-|
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

`wifi scan` on the AX201 is the firmware's passive scan of every allowed
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
CCMP header when it reports the frame as CCMP's) are taken out. No test
boot has the device: CI tests the driver's encodings and its command
sequences against a fake firmware (`cargo test -p iwx`); the rest needs
the laptop.

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
| `myos-open` | `02:00:5e:00:01:01` | 1 | −40 | open |
| `myos-wpa2` | `02:00:5e:00:01:06` | 6 | −55 | WPA2-PSK (CCMP) |
| `myos-far` | `02:00:5e:00:01:0b` | 11 | −82 | WPA2-PSK (CCMP) |

The tests (`user/wifi/test.sh`, tag `wifi`) start the daemon. `wifi_scan`
scans and expects those three lines, from `scan` and `list` alike.
`wifi_connect` joins `myos-open` (`status` names it, aid 1), is refused
`myos-wpa2` without a passphrase (the open network left meanwhile) and
with a wrong one (aid 2, let go), joins `myos-open` again (aid 3), then
`myos-wpa2` with its passphrase (aid 4), and disconnects. `wifi_data`
(tags `wifi,tap`) joins `myos-open`, waits for netd's lease on the
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
