# Sound

Sound in myos has three layers. The card drivers (`virtio_sound`,
`usb_audio`) drive the hardware. The `sound` module is the core: it gives
each of their PCM streams a buffer and a directory under `/dev/sound`.
The sound server, `sndd`, owns those directories and mixes what every
program plays into them. Programs talk to the server over a local
socket, never to `/dev/sound`, because a stream's `data` is held by one
program at a time.

| Piece | What |
|-------|------|
| `modules/sound` | The core: the cards and their streams, `/dev/sound/N/{ctl,data}`, the `sound` service (`SoundOps`) the card drivers register with |
| `modules/virtio_sound` | virtio-sound cards (PCI or MMIO): every playback and capture stream they have |
| `modules/usb_audio` | USB Audio Class 1 and 2 playback (speakers, DACs, audio interfaces), one stream per device (`docs/usb.md`) |
| `user/sndd` | The sound server: mixing, rate, channel and format conversion |
| `user/c/play.c` | `play` (a tone or a WAV file) and `rec` (into a WAV file), in `/bin/etc/` |

## Playing and recording

```sh
play -t 440 -d 2                 # a 440 Hz tone for two seconds
play -r 22050 -c 1 -f u8 -t 440,660 -d 1   # two tones, mono 8-bit
play -D 3 file.wav               # a WAV file (u8 or s16 PCM) on stream 3
rec -r 16000 -c 1 -d 5 out.wav   # five seconds from the first capture stream
rec -d 1 -n                      # only say how many frames came
play -l                          # the server's `devices`
```

## The sound server

Init starts `sndd` in the `sndd` domain (`docs/security.md`). It listens
on the local socket `sndd` (`docs/sockets-unix.md`). A connection starts
with one line, answered with `ok` or `error WHY`:

- `play RATE CHANNELS FORMAT [STREAM]`: the program then writes PCM until
  it closes, to `STREAM` or the first playback stream.
- `record RATE CHANNELS FORMAT [STREAM]`: the server writes what the
  capture stream records until the program closes; what is not read in
  time is dropped.
- `devices`: one line per stream, `N playback|capture CARD`.

The server converts each program's frames to the stream's format, sums
them and clips. A stream runs at 48 kHz stereo `s16` when the card can,
else at the closest it has, and stops half a second after the last
program goes. A program buffers up to 64 KiB in the server; past that its
writes wait in the socket, so `play` keeps pace with the card.

## `/dev/sound`

The core numbers the streams of all cards in the order they registered.
Stream `N` is `/dev/sound/N`:

- `ctl` reads the card's name, direction, channel range, formats, rates,
  `state` (stopped, or running with its parameters) and the underrun and
  overrun counts. It takes `start RATE CHANNELS FORMAT [PERIOD_FRAMES]`
  (a 20 ms period by default) and `stop`.
- `data` is the PCM: interleaved frames, unsigned 8-bit or signed 16-bit
  little endian. One program holds it at a time, and its last close stops
  the stream. Writes and reads take whole frames; `poll` works.

A playback stream short of data plays silence (an underrun); a capture
stream nobody reads drops its oldest data (an overrun). A card that goes
away (a speaker pulled out) takes its streams with it, and a held `data`
fails.

The virtio-sound driver is myos's own, on the `virtio-drivers` crate's
transport, because the crate's driver busy-waits while sound plays and
has no capture.

## In QEMU

Every board with PCI has a virtio-sound card and a `usb-audio` speaker
(`src/sound.rs`); the Raspberry Pi 3 has neither. In a window
(`cargo run`) both play where `MYOS_AUDIODEV` says: a QEMU `-audiodev`
driver (`pa`, `pipewire`, `alsa`, `coreaudio`, `sdl`, ...), `none`
without it.

```sh
MYOS_AUDIODEV=pa cargo run -- uefi
```

### The boot test

A test boot has two virtio-sound cards and three USB speakers (behind
xHCI, UHCI and OHCI, `docs/usb.md`). The first card and the speakers play
into QEMU `wav` files, `target/boot-test-<name>-sound*.wav`; the second
card records QEMU's `none` input, which is silence. `user/sndd/test.sh`
(tag `sound`) plays, mixes, records and lists. After QEMU exits, the
launcher checks the WAV files for the expected tones with a Goertzel
filter (`sound::check_played`, `src/sound.rs`); missing sound fails the
boot. The recording test accepts up to twice the expected frames on a
loaded host, since QEMU's input follows the host's clock.

## Limits

- 8 cards and 16 streams. A stream's buffer is four periods of at most
  64 KiB; virtio-sound periods are at most 16 KiB.
- Formats: unsigned 8-bit and signed 16-bit PCM only. The server takes up
  to 8 channels.
- No volume control, no mixer controls, no channel maps (stereo is left,
  right).
- The server resamples by sample and hold: fine for tones and speech, but
  music at a far rate has audible aliasing.
- The virtio-sound event queue (jack and period notifications) is not
  driven.
- USB Audio: playback only (issue #610) and 16-bit PCM. It needs a host
  controller with isochronous transfers: the xHCI, EHCI, UHCI and OHCI
  have them, the DWC2 does not (issue #608).
- The capture test hears only silence: QEMU's `wav` backend has no input
  (issue #609).
