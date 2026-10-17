# Wi-Fi

Wi-Fi is under way (issue #495): `wifi` is the daemon and its command
(`user/wifi`). Today it scans, through the test boots' simulated radio;
the radio of a real laptop (Intel's AX201, a port of OpenBSD's `iwx`
over `/dev/pci`), association with a network, WPA2 and handing frames
to netd come next.

## `wifi`

| Command | |
|------|-|
| `wifi daemon [NIC]` | drives the radio: the simulated one on `NIC` (default `/dev/net1`); listens on the unix socket `wifid` (`/net/unix`, `docs/sockets-unix.md`) |
| `wifi scan` | a scan of the 2.4 GHz channels; prints the networks that answered |
| `wifi list` | what the last scan found |

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
says its address.

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
the probe requests on them (for their SSID or the wildcard). They beacon
only for 5 seconds after the radio last sent a frame: a boot that never
scans is not woken thirty times a second for nothing.

| SSID | BSSID | Channel | Signal | Security |
|------|-------|---------|--------|----------|
| `myos-open` | `02:00:5e:00:01:01` | 1 | −40 | open |
| `myos-wpa2` | `02:00:5e:00:01:06` | 6 | −55 | WPA2-PSK (CCMP) |
| `myos-far` | `02:00:5e:00:01:0b` | 11 | −82 | WPA2-PSK (CCMP) |

The test (`user/wifi/test.sh`, tag `wifi`) starts the daemon, scans and
expects those three lines, from `scan` and `list` alike.
