# Wi-Fi

Wi-Fi is under way (issue #495). `wifi` (`user/wifi`) is the daemon and its
command. It scans, joins an open or WPA2-PSK network and carries netd's
traffic over it. It drives four kinds of radio:

- the test boots' simulated radio;
- an Intel Wi-Fi 6 radio: a laptop's AX201 or an AX200 card (below,
  "Intel's radios");
- a MediaTek MT7601U or MT7612U USB adapter, which any arch's USB port
  takes ("MediaTek's radios");
- the Raspberry Pi's own Broadcom chip ("Broadcom's radios").

None has been tried on the hardware yet.

## `wifi`

| Command | |
|------|-|
| `wifi daemon [NIC]` | drives the radio (below) and listens on the unix socket `wifid` (`/net/unix`, `docs/sockets-unix.md`) |
| `wifi scan` | a scan; prints the networks that answered |
| `wifi list` | what the last scan found |
| `wifi connect SSID [PASSPHRASE]` | join the network `SSID`: an open one, or a WPA2-PSK one with its passphrase |
| `wifi status` | `associated BSSID CHANNEL SSID aid N`, or `disconnected` |
| `wifi disconnect` | leave the network |

- **`daemon`** picks the first radio it finds, in this order: an AX201 or
  AX200 when `/dev/pci` has one and no `NIC` is named; else an MT7601U or
  MT7612U when `/dev/usb` has one; else a CYW43455 or CYW43438 when
  `/dev/sdio` has one; else the simulated one on `NIC` (default
  `/dev/net1`).
- **`scan`** covers the 2.4 GHz channels of the simulated radio and the
  MT7601U, both bands of the MT7612U, every channel an Intel radio's
  regulatory domain allows, or the channels a Broadcom radio's firmware
  scans.
- **`connect`** joins the strongest network named `SSID` that the last
  scan heard. When the last scan heard none, it scans first. The
  passphrase is 8 to 63 printable ASCII characters, or the PSK itself as
  64 hex digits.
- **`status`** shows `aid 0` on a Broadcom radio, whose firmware keeps
  the association.

A command is one line over one connection to `wifid`. `connect` sends the
passphrase on a second line. The daemon answers and ends the connection.
The passphrase is an argument for now, so `ps` shows it.

A network is a line `BSSID CHANNEL RSSI SECURITY SSID`, the strongest
first:

```
02:00:5e:00:01:01 1 -40 open myos-open
02:00:5e:00:01:06 6 -55 wpa2-psk myos-wpa2
02:00:5e:00:01:0b 11 -82 wpa2-psk myos-far
```

`SECURITY` is one of:

- `open`;
- `wpa2-psk`: RSN with pre-shared keys;
- `wpa2`: RSN with another key management (802.1X, SAE);
- `wep`: privacy without RSN.

Bytes of an SSID outside printable ASCII are written `\xHH`.

## How the daemon works

### Radios

The radio is a trait (`radio.rs`). Tuned to a channel, it sends and hears
802.11 frames, and it says its address. There are three kinds:

- **A SoftMAC radio** (MediaTek's) is tuned and listened to like the
  simulated one.
- **A radio whose firmware scans by itself** (Intel's) hands back the
  frames its scan heard instead.
- **A FullMAC radio** (Broadcom's; `FullMac` in `radio.rs`) scans, joins
  and encrypts in its firmware, and carries Ethernet frames. The daemon
  asks it and waits for its answer. The link's frames go between it and
  the NIC as they are (`daemon_fullmac` in `main.rs`).

The rest of this section is the SoftMAC path.

### Scanning

A scan tunes the radio to each of its channels in turn (`Radio::channels`:
1 to 13, or for the MT7612U the board's bands, 13 + 25 channels). On each
channel it:

1. sends a wildcard probe request, except on a 5 GHz radar channel (52 to
   144), where a station only listens until it hears an access point;
2. listens 120 ms for probe responses and beacons (beacons come every
   102.4 ms);
3. keeps each BSSID once, with its strongest signal.

The code is `user/wifi/src/main.rs`; the frames are in `frame.rs`.

### Joining

Joining (`mlme.rs`) is open-system authentication, then association. Each
is a request and its answer. A request is sent again (three times, 300 ms
apart) when no answer comes.

- A refusal names the step and the status code the access point gave
  (`error: association refused, status 43`).
- `connect` first leaves the network the station is on, with a
  deauthentication.
- An access point's deauthentication or disassociation ends the link.

### Rates and 802.11n

The station offers:

- on 2.4 GHz, the 802.11b and g rates (1 to 54 Mb/s, the faster four as
  Extended Supported Rates);
- on 5 GHz, the 802.11a ones (6 to 54).

The association request also offers 802.11n when both sides do it:

- the radio does 802.11n itself (`Radio::ht`, its HT Capabilities): the
  MT7601U's one stream, the MT7612U's two, and the simulated radio's;
- the access point is an 802.11n one (its beacon's HT Capabilities and
  WMM elements).

The request then carries the radio's HT Capabilities and a WMM Information
element, since an 802.11n station is a QoS one. The link is then a QoS
link:

- The daemon sends QoS data frames of best effort. EAPOL frames go as
  plain data.
- It declines a block ack session the access point offers, answering its
  ADDBA request with status 37. So frames come one at a time.

The radio learns which kind of link it is on (`Radio::associated`). While
associated, it gets a tick a second (`Radio::tick`): the MediaTek radios'
rate adaptation, and the MT7612U's calibration.

### WPA2-PSK

On a WPA2-PSK network the code is `rsn.rs` and the `wpa` crate
(`user/wifi/wpa`). The association request carries an RSN element (CCMP,
PSK). Then the 4-way handshake follows (IEEE 802.11-2020 12.7.6):

1. The pairwise master key is PBKDF2-HMAC-SHA1 of the passphrase and
   SSID.
2. Message 1 brings the access point's nonce.
3. The station answers with its own nonce (from `/dev/urandom`) and its
   RSN element, under a MIC made with the pairwise key both sides now
   derive.
4. Message 3 proves the access point has that key and brings the group
   key (AES key wrap). Its RSN element is checked against the beacon's.
5. Message 4 ends it, and the keys are in.

Three seconds cover the whole handshake. A wrong passphrase shows in one
of two ways:

- the access point lets the station go after message 2 (`error: 4-way
  handshake: the access point refused the key: wrong passphrase?`);
- message 3's MIC is wrong.

From then on every data frame is CCMP's (AES-CCM, an 8-byte MIC; 12.5.3):

- A frame is sent with the pairwise key.
- A frame is received with the pairwise key, or with a group key by the
  index the frame names. Up to three group keys are kept, so a rekey does
  not drop the frames still under the old one.
- A frame whose packet number is not past the last one's is dropped as a
  replay.
- A group key handshake (the access point rekeying the group) is answered
  while the link is up.

The cryptography is RustCrypto's (`THIRD_PARTY_NOTICES.md`). `cargo test
-p wpa` checks the keys, the EAPOL-Key frames and CCMP against IEEE
802.11's PSK vectors, RFC 3394's key wrap, and vectors another
implementation made (Python's `hmac` and `cryptography`).

### The link to netd

While associated, the daemon holds a virtual NIC (`/dev/tap`, below) with
its link up and the radio's address. netd takes it as `tapN` and asks the
network's DHCP for an address (`tap.rs`).

- An Ethernet frame netd sends on it goes out as a data frame to the
  access point. The frame goes to the distribution system (addresses: the
  BSSID, the station, the destination), with an LLC/SNAP header before
  the EtherType.
- A data frame from the access point to the station or a group comes
  back in as Ethernet (`frame.rs`).
- Leaving the network, or the access point letting the station go, takes
  the NIC away. netd lets go of it and its lease.
- A scan while associated tunes the radio back to the link's channel when
  it is done.

## Firmware

A real radio runs its maker's firmware:

| Radio | Files | Named in |
|-------|-------|----------|
| AX201 | `iwlwifi-QuZ-a0-hr-b0-77.ucode` | `iwx::chip` |
| AX200 | `iwlwifi-cc-a0-77.ucode` | `iwx::chip` |
| MT7601U | `mt7601u.bin` (its MCU's) | `mtw::chip` |
| MT7612U | `mt7662.bin` after its ROM patch `mt7662_rom_patch.bin`: the MT7662E's files, which Linux's mt76x2u loads too | `mtw::chip` |
| CYW43455 | `cyfmac43455-sdio.bin`, the regulatory data `cyfmac43455-sdio.clm_blob`, the board's NVRAM `brcmfmac43455-sdio.raspberrypi,3-model-b-plus.txt` or `raspberrypi,4-model-b.txt` | `bwfm::chip`, "Broadcom's radios" below |
| CYW43438 | `cyfmac43430-sdio.bin`, its `.clm_blob`, `brcmfmac43430-sdio.raspberrypi,3-model-b.txt` | `bwfm::chip`, "Broadcom's radios" below |

The images never carry it. Intel's, MediaTek's and Cypress's licences let
the firmware be redistributed unmodified (the Pi's NVRAM files are GPLv2
in linux-firmware), but myos ships no third-party binaries.
`wifi-firmware` gets it:

| Command | |
|------|-|
| `wifi-firmware fetch` | downloads each file from linux-firmware at a pinned commit, checks its SHA-256 (a file that does not match is not kept) and keeps it in the firmware directory; one already there is left alone |
| `wifi-firmware list` | each file, `ok`, `corrupt` or `missing` |
| `wifi-firmware path` | the firmware directory |

The firmware directory is the first of:

1. `$MYOS_FIRMWARE`;
2. `/data/firmware`, when the boot disk's data partition is mounted at
   `/data` (`docs/install.md`). It is still there after a reboot.
3. `/tmp/firmware`.

The daemon reads the firmware from `/data/firmware`, else `/tmp/firmware`.
It does not see `$MYOS_FIRMWARE`.

A first fetch needs a network: wired, or a mirror.
`$WIFI_FIRMWARE_MIRROR` replaces the download's base URL. When it names a
directory with a copy of linux-firmware's tree (a USB stick),
`wifi-firmware` copies from it instead.

The tests are `wifi_firmware_check` (a file with the wrong SHA-256 is
refused) and `wifi_firmware_fetch` (tag `net`: the real download).

## Intel's radios

Two of Intel's Wi-Fi 6 radios, of the same (22000) family. They share one
driver, the same rings, commands and firmware protocol. Each radio's own
facts are in `iwx::chip`:

| Radio | PCI | MAC | | Firmware |
|------|-----|-----|-|----------|
| AX201 | `8086:06f0` (Comet Lake H, the ThinkPad T15p's), `8086:02f0` | QuZ | integrated (CNVi: the MAC in Intel's chipset, so Intel x86 machines only); crystal latency 500 µs | `iwlwifi-QuZ-a0-hr-b0-77.ucode` |
| AX200 | `8086:2723` | CC | discrete: an M.2 PCIe card, for any machine with the slot | `iwlwifi-cc-a0-77.ucode` |

What differs between them, as OpenBSD's iwx_attach sets it up:

- the MAC the PCI id must have (HW_REV);
- whether the hardware start forces power gating and resets again
  (integrated only);
- where the latency tolerance for the firmware's boot goes: the
  periphery's `HPM_UMAC_LTR`, or a discrete radio's `LTR_LONG_VAL_AD`;
- SOC_CONFIGURATION: the `DISCRETE` flag, the crystal latency.

The two firmware files agree in everything the driver checks (command
versions, API and capability bits, limits).

### Access through `/dev/pci`

The daemon drives the radio itself (`user/wifi/src/intel.rs`). It takes
the first radio in `/dev/pci` whose id is in the table, and reaches it
through `/dev/pci` (`docs/pci.md`; `user/wifi/src/pci.rs`):

- BAR0 mapped;
- DMA buffers;
- its MSI-X or MSI interrupt, or its INTx line. All the aarch64 and
  riscv64 boards give a PCI function (an AX200 card's on them) an INTx
  line. It is serviced as MSI's are, then unmasked again through
  `/dev/pci`'s `irq`.

`/dev/pci` is there once root loads its module with
`insmod /lib/modules/pcidev`, a line for `/data/etc/rc.local`. It is not
loaded at boot because on a board without an IOMMU the daemon's device
reaches all of memory. The laptop's VT-d confines it to the daemon's
buffers.

### The `iwx` crate

The driver is the `iwx` crate (`user/wifi/iwx`). It is a port of OpenBSD's
`iwx(4)` (if_iwx.c rev 1.230) to `no_std` Rust behind a `Device` trait, so
it builds and its encodings are tested on the host (`cargo test -p iwx`):

| Module | |
|------|-|
| `chip.rs` | the radios: their ids, MAC, integrated or discrete, crystal latency, firmware file |
| `fw.rs` | the firmware file: its TLVs, the runtime image's LMAC, UMAC and paging sections, API and capability bits, command versions |
| `reg.rs`, `hw.rs` | registers; the NIC lock, the periphery window, preparing, resetting and starting the hardware, interrupt masks, the MAC address |
| `ring.rs` | the context information the firmware loads itself from, the command queue's descriptors and commands, the packets of a receive buffer |
| `msg.rs` | the commands up to a scan and the answers and notifications read (ALIVE, the NVM, the regulatory domain, the scan, a received frame) |
| `sta.rs` | joining as a station: the PHY, MAC and binding contexts, the station and its transmit queue, the session protection, the TX command and its response, the beacon timing the firmware needs |
| `dev.rs` | the bring-up: memory shared with the firmware, its load up to ALIVE, the interrupt (ICT or MSI-X causes), commands and their answers, the configuration; joining, sending, leaving |

### Bring-up

Bring-up is one pass, as the 22000 family's unified firmware wants:

1. The hardware is prepared and started.
2. The firmware's sections are copied into DMA memory and named in the
   context information.
3. The self load is started, and the driver waits for ALIVE (1 s).
4. Then come, in order: the NVM access commands; the NVM itself (the
   SKU's bands, the antennas, the channels); the antennas; Bluetooth
   coexistence off; the SoC's crystal latency; the device kept awake.
5. The regulatory domain is asked of the firmware's location-aware
   regulatory, world `ZZ` to start. Its answer replaces the NVM's
   channels.
6. The scan configuration.

The DMA memory is eleven buffers of `/dev/pci`'s sixteen:

- control (context information, ICT, receive descriptors and status);
- the command queue's descriptors;
- its commands;
- the 512 receive buffers, in four;
- the firmware's two images (the paging one stays the device's while it
  runs);
- at the first join, the station's transmit queue: its 256 descriptors
  with the byte count table, and 64 slots of 4 KB for the frames on their
  way.

### Scanning and joining

`wifi scan` on an Intel radio is the firmware's passive scan of every
allowed channel (110 ms each), with every beacon and probe response passed
up. The daemon reads the networks from them as from the simulated radio's.
It keeps each access point's beacon timing (interval, DTIM, timestamp) and
rates for the firmware.

Joining is the daemon's, as with the simulated radio (`mlme.rs`,
`rsn.rs`). The radio is told each step (`Radio::joining`, `associated`,
`left`).

Before authentication the firmware gets the network (iwx_auth):

- a PHY context on its channel (20 MHz) and its receive chains;
- a MAC context: a BSS station of the BSSID, beacons and group frames up;
- their binding;
- the access point as station 0;
- its transmit queue (SCD_QUEUE_CONFIG, queue 1, the management TID);
- a session protection that keeps it on the channel for the
  authentication and association (at least 2000 TU).

The daemon's frames then go out on that queue:

- Each is a TX command with its 802.11 header (padded to 4 bytes), and
  the body in a transfer buffer of its own.
- Management and group frames go at the lowest rate (1 Mb/s, 6 on
  5 GHz).
- EAPOL goes ahead of the rest.
- A frame's response frees the slot and says whether the frame was
  acknowledged.

Once associated (iwx_run):

- the station and the MAC context are updated (associated, its id, the
  DTIM timing from the beacon);
- the smart FIFO is on;
- every group frame is passed up;
- the device is kept awake;
- the firmware's rate scaling runs over the access point's non-HT rates.

Leaving (iwx_run_stop, iwx_deauth) drains and flushes what was queued.
Then it removes the queue, the station, the binding and the contexts.

### The link

It is a non-HT station:

- legacy rates up to 54 Mb/s;
- no QoS: the association request asks for none, so the access point
  sends plain data frames;
- no aggregation.

On a WPA2 network the daemon encrypts and decrypts as it does over the
simulated radio (CCMP in software):

- No key is given to the firmware.
- Every frame is sent with its encryption off (TX_FLAGS_ENCRYPT_DIS), as
  OpenBSD sends the frames it encrypts itself.
- The protected frames the radio receives come up as they were sent.
- The firmware puts two bytes of padding after a frame's header, or after
  its CCMP header when it reports the frame as CCMP's. The driver takes
  them out.

### Tests

No test boot has either radio. `cargo test -p iwx` (CI runs it) tests the
driver's encodings. It also runs the driver against a model of each radio
(`user/wifi/iwx/tests/model`, `radios.rs`; what differs between them is in
its `Hardware`). The model has:

- the registers and periphery;
- the interrupt (MSI with the ICT, MSI-X with its automask);
- the receive ring and transmit queues in the driver's DMA buffers;
- a firmware that loads itself from the context information, checking
  the sections are those of `iwx::sim` (a synthetic file in Intel's
  format with myos's own bytes). It sends ALIVE and answers every command
  of the bring-up, the scan and joining a network as the real one does.
- three access points (the simulated radio's, below) that beacon and
  answer authentication and association.

Every command, descriptor and register access is checked against the
firmware's structures as OpenBSD's if_iwxreg.h describes them. The first
thing a real device would not take stops the model with the reason. This
includes the radio's own facts: its MAC, where it takes the latency
tolerance, its SoC configuration.

The tests:

- bring each radio up with MSI and with MSI-X;
- scan, hearing the three networks at their signals;
- join `myos-open`: authenticate, associate and leave;
- check that an AX200 the driver takes for integrated is refused.

What the model cannot show is whether Intel's firmware agrees with that
reading of it. The rest needs the hardware.

## MediaTek's radios

| Radio | What | Examples |
|-------|------|----------|
| MT7601U | an 802.11b/g/n USB adapter for 2.4 GHz with one antenna: the cheap "150 Mbps" dongles | ASUS USB-N10 v2, D-Link DWA-127 rev B1, Edimax EW-7711UAn V2, Xiaomi's and many unbranded ones |
| MT7612U | an 802.11ac USB 3 adapter for 2.4 and 5 GHz with two antennas: the "AC1200" ones | ASUS USB-AC55, Netgear A6210, Alfa AWUS036ACM, TP-Link TL-WDN6200, the Xbox One adapter |

With one of these, a machine of any arch with a USB port can have Wi-Fi:
a Raspberry Pi, a RISC-V board. The MT7612U is driven as an 802.11n radio
of two streams at 20 MHz. 40 and 80 MHz and 802.11ac are issue #563.
Their ids and firmware are in `mtw::chip`.

The MT7610U is not supported. The only complete driver for it, Linux's
mt76x0u, is GPL-2.0-only, and OpenBSD's mtw(4) does not take it.

### Access through `/dev/usb`

Each is a SoftMAC. Its MCU calibrates the radio and takes the frames, and
the host does the rest. So the daemon scans (tuning to each channel and
sending probe requests, as for the simulated radio), joins and encrypts.
The radio is told the network it joins (`Radio::joining`, `associated`,
`left`).

The daemon drives it (`user/wifi/src/mediatek.rs`) through `/dev/usb`
(`docs/usb.md`; `user/wifi/src/usb.rs`), which is there at boot. It takes
the first device there whose id is in the table, and holds its `ctl`:

- Its registers are read and written with vendor requests on `ep0`. A
  32-bit register is two `WRITE_2`s.
- The frames heard are read from its first bulk IN endpoint. A read is
  one transfer, which the MAC fills with up to 2 KiB of frames.
- The MCU's commands, the firmware and the management frames are written
  to its first bulk OUT endpoint.
- The data frames go to the next bulk OUT endpoint (best effort; there is
  one per access category).

### The `mtw` crate

The driver is the `mtw` crate (`user/wifi/mtw`), behind a `Usb` trait:

- for the MT7601U, a port of OpenBSD's `mtw(4)` (if_mtw.c rev 1.13) to
  `no_std` Rust;
- for the MT7612U (`mt76x2/`, below), a port of Linux's mt76x2u.

| Module | |
|------|-|
| `chip.rs` | the radios: their USB ids, ASICs, driver, firmware files and where they go, HT Capabilities |
| `fw.rs` | the firmware file: its header, interrupt vectors, instruction and data parts |
| `reg.rs` | registers, vendor requests, the eFUSE ROM's words, the descriptors' flags, the MAC, baseband and RF defaults, the channels' RF settings, the legacy rates |
| `usb.rs` | the `Usb` trait (vendor and class requests, bulk OUT, the MCU's answers on bulk IN) and which bulk endpoint is which |
| `dev.rs` | the bring-up: the WLAN core powered, the firmware loaded and started, the eFUSE ROM read (address, calibration), the MAC, baseband and RF set up and calibrated; channels, joining, sending, leaving, stopping |
| `rx.rs`, `tx.rs` | the frames as the endpoints carry them: an aggregated transfer's frames after their RXWI (signal, flags; a padded header put back), a frame to send after its TXD and TXWI (rate, ACK, the station's entry) |
| `rate.rs` | the link's rates and AMRR (both radios') |
| `mt76x2/` | the MT7612U: `reg.rs` (registers, vendor requests, the EEPROM's layout, MCU messages), `fw.rs` (the ROM patch and firmware files), `frame.rs` (its RXWI and TXINFO/TXWI), `dev.rs` (the radio) |

### The MT7601U's bring-up

Bring-up follows mtw_attachhook and mtw_init:

1. The WLAN core and its oscillator are turned on.
2. The firmware is written in 8 KiB blocks to the MCU's memory, its
   interrupt vectors last. A firmware already running is kept: that is a
   daemon started again without the device reset.
3. The eFUSE ROM is read: the address, the frequency offset, each
   channel's transmit power, the rate groups' power, the LNA gain and
   signal offset.
4. The MAC is reset and given its defaults and address. The key tables
   are cleared.
5. The radio is turned on. The RF banks and the baseband get their
   defaults.
6. The MCU calibrates the RF: frequency offset, temperature, VCO, RX DC.
7. Channel 1 is tuned.
8. Then DMA, receive aggregation and a station's receive filter: frames
   to another station, control frames, duplicates and damaged ones are
   dropped.

A channel is its four RF registers, its transmit power, the LNA and a VCO
calibration.

### Sending, receiving and rates

The daemon's frames go out as they are. The MAC puts nothing in them
(`TXD_WIV`: the IV is the frame's, since CCMP is the daemon's).

- A management or group frame goes at 1 Mb/s.
- A data frame to the access point goes as the access point's entry in
  the station table, which association sets, at the rate AMRR chose.

AMRR (`rate.rs`, from OpenBSD's ieee80211_amrr.c) picks among the rates
both ends have, slowest first:

- on an 802.11n link, MCS 0 to 7 (one stream, 20 MHz, a long guard
  interval: 6.5 to 65 Mb/s);
- on another link, the access point's legacy rates.

AMRR starts at the slowest. Once a second (`Radio::tick`) it reads the
MAC's TX statistics (successes, retries, failures; cleared when read):

- A second with more than 10 frames and under a tenth retried goes up a
  rate.
- A second with over a third retried goes down a rate.
- An increase that fails right away makes the next wait twice as long,
  up to 10 seconds.
- In between, the hardware falls back on its own on a frame's retries
  (OFDM down to 6 Mb/s, CCK down to 1).

A frame wanting an ACK gets the ACK's duration at its control rate.

The frames heard come up as the access point sent them, since no key is
in the MAC. Their signal is in dBm, from the ROM's offsets.

Once associated, the radio gets the slot time and CCK preamble the access
point's capabilities allow, its basic rates and the BSSID. The beacon
timer (TSF sync) stays off, since the station never sleeps.

### The MT7612U

Its driver (`mtw::mt76x2`) is a port of Linux's mt76x2u, and of the
mt76x02 and mt76 code it calls, at 20 MHz.

- Its registers are reached by `MULTI` vendor requests. The USB
  configuration's and the EEPROM's have their own requests.
- Its MCU's commands carry a sequence number and are answered on the
  second bulk IN endpoint (read for up to 1.5 s). A calibration left
  unanswered is no failure, as Linux takes it; the tuning itself is.
- A frame goes out after a TXINFO and a 20-byte TXWI, its header padded
  to 4 bytes. Management frames go on the voice endpoint.
- One frame comes in per transfer (receive aggregation off).

Bring-up follows mt76x2u_init_hardware:

1. The 512-byte EEPROM is read.
2. The WLAN core is reset and powered (its power domain, both RF units).
3. The ROM patch is written through the FCE in 2 KiB blocks and applied.
4. The firmware's instructions and data are written in 14 KiB blocks,
   then started on its vectors.
5. USB DMA is set up without receive aggregation. The queues are selected
   and the radio is turned on.
6. The MAC gets its reference defaults, and the crystal's trim from the
   EEPROM.
7. The station table and the shared keys are cleared.
8. The MCU's RF and baseband settings are loaded, and both chains are
   turned on.

`init` starts the MAC with a station's receive filter and tunes the first
channel.

A channel (mt76x2u_phy_set_channel) is:

- the band's gains and signal offsets, PA settings and per-rate transmit
  power from the EEPROM (two chains, under 20 dBm);
- the band and 20 MHz in the baseband;
- the MCU's tuning and its gain;
- its RX DC calibration, and once, the R and RC ones.

A scan stops there. Joining calibrates the channel too, as Linux does
outside a scan: LC on 5 GHz, TX LO and IQ, RX IQ, temperature, TX
shaping, TSSI. A radar channel is the exception and is not calibrated.

Once a second (`calibrate`), besides AMRR, the driver runs:

- the channel's calibration, if a scan left it undone;
- the TSSI compensation: triggered, then the chains' slopes and offsets,
  then a DPD calibration once;
- the receive gain for the access point's averaged signal: lowered for a
  strong one, and the VGA's adjusted to the false CCAs.

AMRR's rates over 802.11n are MCS 0 to 7, then 12 to 15: 6.5 to 130 Mb/s.
MCS 8 to 11 are skipped because at two streams they are as fast as 2 to
6. On 5 GHz the slowest rate is 6 Mb/s and there is no CCK.

A frame's signal is the two chains' combined (mt76_rx_signal), after the
EEPROM's offsets and LNA gain.

### Tests

No test boot has the radio: QEMU has no USB Wi-Fi device. `/dev/usb`
itself is tested in the boots (`usbdev`, with a USB serial adapter:
`docs/usb.md`).

`cargo test -p mtw` (CI runs it) tests the encodings: the firmware header,
the endpoints, aggregated receive transfers, the transmit descriptors.

It also runs the driver against a model of the MT7601U
(`user/wifi/mtw/tests/model`, `radio.rs`). The model has:

- its registers behind the vendor requests;
- the eFUSE ROM;
- the RF and baseband windows (a temperature reading and the RX DC
  calibration finishing as the real ones do);
- the MCU, which takes a synthetic firmware file in blocks and starts on
  its vectors only when its code is where it belongs, and its commands;
- the bulk endpoints: frames out to the air on the channel the RF
  registers and VCO calibration tuned to, and back in aggregated as the
  MAC does, past its receive filter;
- the air of the Intel model (the three access points).

Every request and descriptor is checked against OpenBSD's mtwreg.h. The
first one a real device would not take stops the model with the reason:
for example a frame sent with the MAC's transmit off, a data frame whose
station entry is not its access point, the firmware written before the
MCU's DMA is set up, or a PHY mode or MCS the chip does not have.

The model's air carries a frame only as fast as the access point's signal
allows: all of 802.11n at −40 dBm, 9 Mb/s at −82. A frame sent faster is
retried 7 times and fails, as the TX statistics then count.

The MT7601U tests:

- bring the radio up, with the calibrations in order;
- keep a running firmware;
- refuse another chip;
- scan the three channels, each network at its signal;
- join `myos-open`, send to it and leave;
- check AMRR: over 802.11n there it climbs from MCS 0 to MCS 7 in seven
  seconds and stays. On `myos-far` (802.11g, −82 dBm) it climbs to 9 Mb/s
  and tries 11 now and then, more rarely each time.

The MT7612U's tests (`user/wifi/mtw/tests/mt76x2.rs`) run its driver
against a model of it (`tests/model/mt76x2.rs`). The model has:

- the MAC's registers;
- the configuration space (the power domain coming up);
- the EEPROM (both bands, internal PAs, TSSI on);
- the FCE, which takes the ROM patch and firmware in blocks: its DMA
  address and length, the descriptor index moved once a block;
- the MCU, which applies the patch, starts only when the ILM and DLM are
  where they belong, and answers the commands that want it with their
  sequence number;
- the frames: out on the channel the MCU tuned to (a TXINFO and TXWI
  checked, the header pad, CCK refused on 5 GHz), and in one frame a
  transfer at two chains' signal;
- a fourth access point in the air, `myos-5g` on channel 36.

The MT7612U tests:

- bring it up: the MCU's commands and calibrations in order, the chains'
  power from the EEPROM;
- refuse another chip;
- scan both bands, a scan's channel not calibrated;
- join `myos-5g` (5 GHz's calibrations, 6 Mb/s) and leave;
- climb with AMRR from MCS 0 to 15 on `myos-open`, and stay at 9 Mb/s or
  under on `myos-far`;
- run the periodic calibration (TSSI, DPD, the gain lowered 14 for
  −40 dBm);
- keep a radar channel uncalibrated.

## Broadcom's radios

The Raspberry Pi's Wi-Fi is a Broadcom (Cypress's, now Infineon's) FullMAC
chip on the SoC's second SDHCI controller:

| Chip | What | Boards |
|------|------|--------|
| CYW43455 | dual band 802.11ac | 3 B+, 3 A+, 4, 400, Compute Module 4 |
| CYW43438 | 2.4 GHz 802.11n | 3 B, Zero W |

The chip runs the whole 802.11 state machine in its firmware. It scans,
joins, runs the WPA2 handshake with its own supplicant, keeps the keys and
encrypts. It exchanges Ethernet frames with the host.

### Access through `/dev/sdio`

The daemon (`user/wifi/src/broadcom.rs`) drives the first such card in
`/dev/sdio`, through `user/wifi/src/sdio.rs`. The `sdmmc` module hands
SDIO cards over that way ("SDIO" in `docs/raspberry-pi.md`). The module
also raises `WL_REG_ON`, through the VideoCore firmware's GPIO expander.

- The daemon holds the card's `ctl`.
- Function N's registers are read and written through `fN`, with the file
  position as the register address. One byte is a CMD52; more is a CMD53.
  A transfer longer than byte mode's 512 bytes goes as whole blocks first,
  as every host splits them.
- The firmware's frames go through `f2fifo` (a fixed address).
- The block sizes are set through `ctl`: 64 bytes on function 1, 512 on
  function 2.
- The daemon waits for the card's interrupt on `irq`. The kernel masks it
  until the daemon has served the card and writes to `irq` again. A slot
  with no interrupt line is looked at every 20 ms.

The chips' ids and files are in `bwfm::chip`. The firmware, the CLM blob
(the regulatory data the firmware needs) and the board's NVRAM (its
calibration and pins) come from `wifi-firmware fetch` (above). The NVRAM
file is chosen by the start of `/proc/platform`'s `model`, with the boards
sharing files as linux-firmware links them:

- the 3 A+ takes the 3 B+'s;
- the Zero W takes the 3 B's;
- the 400 and the Compute Module 4 take the 4's.

### The `bwfm` crate

The driver is the `bwfm` crate (`user/wifi/bwfm`). It is a port of
OpenBSD's `bwfm(4)` and its SDIO attachment (bwfm.c rev 1.118,
if_bwfm_sdio.c rev 1.45) to `no_std` Rust, behind an `Sdio` trait
(`sdio.rs`: single bytes and transfers on the functions, the block sizes,
time):

| Module | |
|------|-|
| `chip.rs` | the chips (id, name, firmware files, SDIO product), the backplane: the AI interconnect's enumeration ROM read through chipcommon for the cores (the SDIO device core, the ARM CR4 or CM3 and its memory, the 802.11 core) and their agents, the RAM's size and base (the CR4's TCM banks, the CM3's SOCRAM), the PMU, the cores held in reset and released |
| `bus.rs` | the card: function 2 enabled, the backplane window (function 1's SBADDR registers, 32 KiB pages), the clocks (ALP, HT, the save-restore engine), RAM reads and writes through the window, SDPCM: the hardware header (length and its complement), the software header (sequence, channel, data offset, the credits the firmware grants), frames in and out on function 2, glommed frames, the mailbox |
| `bcdc.rs` | BCDC: a command (id, length, flags, status) and its variables (name, NUL, value), the data header before an Ethernet frame |
| `fw.rs` | the firmware's reset vector, the NVRAM text converted to the firmware's form (comments dropped, each setting NUL-terminated, the length token at the end), the CLM blob cut into `clmload` pieces |
| `event.rs` | the firmware's events (an Ethernet frame to Broadcom's EtherType with the event message after a BCM header): their kinds, status and reason, the escan results' BSS records (BSSID, SSID, chanspec to channel, RSSI, capabilities, information elements), the event mask |
| `dev.rs` | the driver: attach (the chip found and held), start (the firmware at the RAM's start and the NVRAM at its end, the CPU released, the firmware's `FWREADY`, function 2 ready, the version, MAC and band list asked, the CLM loaded, the events wanted, power save and the supplicant set, `up`), commands and their answers (one at a time, the answer matched by request id, a second's wait), service (the mailbox, every frame: control answers, events, data), scan, join, leave, send and receive |

### Bring-up

Bring-up follows bwfm_attach, bwfm_sdio_attach, bwfm_sdio_load_microcode
and bwfm_preinit:

1. The backplane window is set.
2. The chip is identified through chipcommon, and its cores are
   enumerated.
3. The CPU is held in reset.
4. The firmware is written to the RAM; for a CR4 chip, the reset vector
   goes at the RAM's start.
5. The NVRAM is written to the RAM's end, with the firmware's length
   token.
6. The CPU is reset with the reset vector (the CR4), or the SOCRAM's
   banks are set up and the CM3 released.
7. The driver waits for `FWREADY` in the mailbox, and for function 2 to
   be ready.
8. The interrupt is enabled at the card (the CCCR's master and function
   enables).
9. The firmware is set up, in order:
   - `cur_etheraddr`;
   - the version (the I/O type decides the chanspec encoding);
   - the band list;
   - `event_msgs`, with the events used;
   - the CLM, in pieces of 1400 bytes;
   - `mpc` and power save (fast);
   - `sup_wpa` 1 (the firmware's supplicant);
   - `up`.

A command is a BCDC message on the control channel with its request id.
The daemon waits for its answer, serving the frames that come first
(events, data).

Credits: a frame is sent only while the firmware's `tx_max` is past the
sequence sent. Each frame received brings the new maximum.

### Scanning and joining

`wifi scan` is the firmware's escan: every channel, active, with the
firmware's own dwell times, both bands on the CYW43455.

- The results come as `ESCAN_RESULT` events: partial ones with BSS
  records, then the one that ends the scan.
- The daemon keeps each BSSID at its strongest.
- It builds the networks from the records' capabilities and elements, as
  it does from a beacon.

`connect`:

1. sets `wpa_auth`, `wsec` and `auth` for the network: none, or WPA2-PSK
   with CCMP and open authentication;
2. gives the firmware the pairwise master key (`wsec_pmk`, the PSK as 64
   hex digits). The daemon derives it from the passphrase as for the
   other radios.
3. asks the `join` iovar with the SSID, the BSSID and the channel's
   chanspec. On a firmware without it, it uses `C_SET_SSID`.
4. waits for the events: `SET_SSID` (its status tells when the network
   is not there), `LINK` up, and on a WPA2 network `PSK_SUP` with the keys
   in place.

If the access point lets the station go before that, the daemon reports
the link as lost, probably a wrong passphrase. That is a deauthentication,
reason 15, after a failed handshake. Twelve seconds cover the join.

While the link is up:

- Ethernet frames go out with a BCDC data header on the data channel,
  and come in from it.
- A frame that comes while the link is down is dropped.
- A `LINK` down or a deauthentication ends the link, and the NIC goes
  with it.
- `disconnect` is `C_DISASSOC`.

The firmware does the encryption, so `rsn.rs` is not involved.

### Tests

No test boot has the radio. QEMU's Raspberry Pi 3 has the SDHCI controller
but no card in it (the `sdio` test checks `/dev/sdio` stays empty), and its
property mailbox has no GPIO expander.

`cargo test -p bwfm` (CI runs it) tests the encodings: BCDC messages and
variables, the NVRAM conversion, the CLM pieces, the events and BSS
records, the chanspecs.

It also runs the driver against a model of the chip
(`user/wifi/bwfm/tests/model`, `radio.rs`). The model has:

- the CCCR and function 1 registers, with the clock request bits
  answered;
- the cores and agents behind the backplane window: a CYW43455 with a CR4
  and its TCM, or a CYW43438 with a CM3 and SOCRAM;
- the RAM;
- a firmware that starts when its CPU is released with its image in
  place, then answers the commands. As the real one does, it sends one
  reply first, and its events once the host has drained the queue.
- a made-up air of an open and a WPA2 network, which the firmware scans
  and joins (the WPA2 one with the right key only). It carries frames
  both ways and lets the station go.

The model checks every frame and command it is given: padding, the
hardware header's checksum, the data offset, the BCDC version, the escan
parameters, the join's chanspec. It found the data offset of transmitted
frames pointing past the BCDC header.

The tests:

- attach to both chips;
- start the firmware: the image and NVRAM where the firmware finds them,
  the set-up commands in order;
- refuse a firmware that does not fit;
- scan, join the open network and move frames;
- join the WPA2 network with the firmware's supplicant, and leave;
- fail with the wrong key, and on a network that is not there.

The kernel's SDIO layer is untried on hardware too, like the rest of the
Pi 4 (`docs/raspberry-pi.md`).

## The simulated radio

QEMU has no Wi-Fi device, so the test boots get a simulated one. It is a
second virtio-net NIC (`/dev/net1`; netd keeps `/dev/net0`) on a QEMU
`socket` netdev. Its UDP datagrams go to a thread of the launcher
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

The radio hears only the frames of the channel it is tuned to.

### The access points

| SSID | BSSID | Channel | Signal | Security |
|------|-------|---------|--------|----------|
| `myos-open` | `02:00:5e:00:01:01` | 1 | −40 | open, 802.11n (HT, WMM) |
| `myos-wpa2` | `02:00:5e:00:01:06` | 6 | −55 | WPA2-PSK (CCMP) |
| `myos-far` | `02:00:5e:00:01:0b` | 11 | −82 | WPA2-PSK (CCMP) |

The launcher's access points:

- beacon on their channels every 100 TU, and answer the probe requests on
  them (for their SSID or the wildcard);
- take open-system authentication and association. A WPA2 one refuses an
  association request without an RSN element (status 43).
- give each association the next association id;
- let a station go at its deauthentication;
- beacon only for 5 seconds after the radio last sent a frame, so a boot
  that never scans is not woken thirty times a second for nothing.

A WPA2 access point (passphrase `myos-passphrase`):

- is the authenticator of the 4-way handshake. When message 2's MIC is
  wrong, it sends a deauthentication, reason 15.
- encrypts and decrypts the data frames with CCMP;
- starts a group key handshake (a new group key) at a station's first
  data frame after the handshake, as an access point rekeying does. Its
  next group frames still go out under the old key until the station
  confirms.

An associated station's data frames reach a router behind each access
point (`src/wifi_net.rs`). The router:

- is 10.0.8.1/24, the access point's address;
- leases the station 10.0.8.(1 + its association id) by DHCP;
- answers ARP for 10.0.8.1, and the pings sent to it.

### Tests

The tests (`user/wifi/test.sh`, tag `wifi`) start the daemon.

- `wifi_scan` scans and expects the three lines above, from `scan` and
  `list` alike.
- `wifi_connect`:
  1. joins `myos-open` (`status` names it, aid 1);
  2. is refused `myos-wpa2` without a passphrase (the open network is left
     meanwhile);
  3. is refused it with a wrong passphrase (aid 2, let go);
  4. joins `myos-open` again (aid 3);
  5. joins `myos-wpa2` with its passphrase (aid 4);
  6. disconnects.
- `wifi_data` (tags `wifi,tap`) joins `myos-open` over 802.11n. The
  simulated radio offers HT, and the access point drops a plain data
  frame from an 802.11n station, so the link's data frames must be QoS
  data. The test waits for netd's lease on the link's NIC and pings
  10.0.8.1 across it. It checks that `net0` keeps the default route, and
  that the NIC goes at `disconnect`.
- `wifi_wpa2_data` does the same over `myos-wpa2`, so the lease comes
  under both group keys and the ping under the pairwise one.

## Virtual NICs (`/dev/tap`)

The link the daemon brings up reaches netd through a virtual NIC
(`modules/tap`), as Linux's TAP devices do: a NIC whose wire is a program.

NIC `N` (0 to 3) is there while a program holds `/dev/tap/N/prog`:

- Opening `prog` makes the NIC. It takes one holder at a time, root only.
- Its last close takes the NIC away.

| File | |
|------|-|
| `prog` | the program's end: a write is one Ethernet frame for netd, a read one frame netd sent (it waits for one; `poll` says one is there) |
| `data` | the NIC's end, as `/dev/netN/data` is: a write sends a frame, a read takes one (0 bytes when none waits), `poll` says one is there |
| `ctl` | `mac ADDR`, `irq on`, `link up` or `link down` and `program N` (the count of programs that have held `prog` so far, this one included), as lines; the program writes `mac ADDR`, `up` and `down` (the association) |

- A frame netd sends while the link is down is dropped.
- Each direction queues up to 64 frames. Past that the newest is dropped,
  as a full NIC drops.
- The test is `tap_smoke` (`user/c/test.sh`, tag `tap`).

netd takes a NIC whose link is up, with its own DHCP lease. It lets go of
the NIC when:

- its link goes down;
- its program goes;
- its program changed since netd last looked, within the second between
  two looks. The `program` count says so, and netd takes the NIC again as
  the new one it is, with a lease of its own (issue #649).

The wired `net0` keeps the default route while it has a lease
(`docs/net.md`, `multihome_smoke`).

## Limits

- No radio has been tried on the hardware. The kernel's SDIO layer is
  untried on hardware too.
- The passphrase is a command argument, so `ps` shows it.
- WPA3 (SAE), 802.1X, TKIP and management frame protection are not
  supported.
- Intel's radios are non-HT stations: no QoS, no aggregation.
- The MT7612U runs at 20 MHz as an 802.11n radio: no 40 or 80 MHz and no
  802.11ac (issue #563).
- The MT7610U is not supported (above).
- The Zero 2 W's CYW43436 (a later revision of the 43430, with firmware of
  its own) and the Pi 5 are not covered.
