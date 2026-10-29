# Sound

Sound in myos has three layers. The card drivers (`virtio_sound`,
`usb_audio`) drive the hardware. The `sound` module is the core: it gives
each of their PCM streams a buffer and a directory under `/dev/sound`.
The sound server, `sndd`, owns those directories and mixes what every
program plays into them. Programs talk to the server over a local
socket, never to `/dev/sound`, because a stream's `data` is held by one
program at a time. `play` and `rec` (`user/c/play.c`, in `/bin/etc/`)
are the server's clients.

| Piece | What |
|-------|------|
| `modules/sound` | The core: the cards and their streams, `/dev/sound/N/{ctl,data}`, each stream's buffer, the `sound` service (`SoundOps`) the card drivers register with |
| `modules/virtio_sound` | virtio-sound cards (virtio 1.2, device 25; PCI or MMIO): every playback and capture stream they have |
| `modules/usb_audio` | USB Audio Class 1 and 2 playback (speakers, DACs, audio interfaces): one playback stream over the streaming interface's isochronous OUT endpoint, 16-bit PCM widened to the device's slot (`docs/usb.md`) |
| `user/sndd` | The sound server: the `sndd` socket, mixing, rate, channel and format conversion |
| `user/c/play.c` | `play` (a tone or a WAV file) and `rec` (into a WAV file), the server's clients |

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
on the local socket `sndd` (`/net/unix`, `docs/sockets-unix.md`). A
connection starts with one line, which the server answers with `ok` or
`error WHY`:

- `play RATE CHANNELS FORMAT [STREAM]`: the program then writes PCM in
  that format until it closes. It goes to `STREAM`, or to the first
  playback stream when there is none.
- `record RATE CHANNELS FORMAT [STREAM]`: the server writes what the
  capture stream records, converted, until the program closes. Without
  `STREAM` it is the first capture stream. What the program does not
  read in time is dropped.
- `devices`: one line per stream, `N playback|capture CARD`.

Everything playing on a stream is mixed. The server converts each
program's frames to the stream's format, sums them and clips the result:

- rate: sample and hold, with a 32.32 fixed-point step;
- channels: mono goes to every channel, any layout goes to mono by
  averaging, otherwise the channels both sides have;
- format: `u8` or `s16`.

A stream runs while a program uses it. It runs at 48 kHz stereo `s16`
when the card can, else at the closest the card has. It stops half a
second after the last program went. A program buffers up to 64 KiB in
the server. Past that its writes wait in the socket, so `play` keeps
pace with the card.

## `/dev/sound`

A card driver registers its card with its streams (`card_register`): a
name and, per stream, its direction, channel range, formats and rates.
The core numbers the streams across all cards in the order they came.
Stream `N` is `/dev/sound/N`, with two files:

- `ctl` reads as lines:
  - `card NAME` (`virtio-sound 00:04.0`, `usb-audio 46f4:0002`)
  - `direction playback|capture`
  - `channels MIN-MAX`
  - `formats u8 s16`
  - `rates ...`, from the virtio-sound list of 5512 to 384000 Hz
  - `state stopped` or `state running RATE CHANNELS FORMAT period BYTES
    buffer BYTES`
  - `underruns N`, `overruns N`

  It takes `start RATE CHANNELS FORMAT [PERIOD_FRAMES]` (a 20 ms period
  without one) and `stop`.
- `data` is the PCM: interleaved frames, unsigned 8-bit or signed 16-bit
  little endian. One program holds it at a time, and its last close
  stops the stream. On a playback stream, a write takes the whole frames
  that fit in the buffer (four periods), and `poll` reports writable
  while a period fits. On a capture stream, a read returns what was
  recorded; it waits while the stream runs and nothing is there.

The driver takes what a playback stream plays one period at a time
(`playback_take`). It gets silence for what the buffer lacks, which
counts as an underrun. It hands over what a capture stream recorded
(`capture_put`). When nobody reads, the oldest data is dropped, which
counts as an overrun. A card that goes away (a speaker pulled out) takes
its streams with it (`card_remove`): their files disappear, and a held
`data` fails.

### virtio-sound

The virtio-sound driver is myos's own. The `virtio-drivers` crate's
driver busy-waits for as long as the sound lasts and has no capture.
myos uses the crate's transport and split queues, and drives the rest
itself:

- the control queue, for `PCM_INFO`, `SET_PARAMS`, `PREPARE`, `START`,
  `STOP` and `RELEASE`;
- four periods in flight on the tx or rx queue per running stream.

A kernel thread per card refills or drains each period as it comes back,
woken by the device's interrupt.

## In QEMU

Every board with PCI has two sound devices (`src/sound.rs`): a
virtio-sound card and a `usb-audio` speaker on the xHCI's root port 5.
The Raspberry Pi 3 has neither. In a window (`cargo run`) both play
where `MYOS_AUDIODEV` says. It names a QEMU `-audiodev` driver (`pa`,
`pipewire`, `alsa`, `coreaudio`, `sdl`, ...); without it the driver is
`none`.

```sh
MYOS_AUDIODEV=pa cargo run -- uefi
```

### The boot test

A test boot has two virtio-sound cards and three USB speakers: the
xHCI's, and one behind a hub on each of a UHCI and an OHCI companion
(`docs/usb.md`).

- The first card (playback only) and the speakers play into WAV files of
  QEMU's `wav` backend at 48 kHz stereo:
  `target/boot-test-<name>-sound.wav`, and `.sound.usb.wav`,
  `.sound.uhci.wav`, `.sound.ohci.wav`.
- The second card records QEMU's `none` input, which is silence.

`user/sndd/test.sh` (tag `sound`) runs these tests:

| Test | What it checks |
|------|----------------|
| `sound_devices` | the streams and the server's list |
| `sound_play` | plays a tone; the stream runs at 48 kHz stereo while it plays and stops after |
| `sound_mix` | two programs at once, at different rates and formats |
| `sound_rec` | records two seconds at 16 kHz mono: about the right number of frames, all silent |
| `sound_usb` | plays through every speaker |

`sound_rec` accepts up to twice the expected frames on a loaded host:
QEMU's input follows the host's clock, while `rec` follows the guest's.

Once QEMU is gone, the launcher reads the WAV files. It runs a Goertzel
filter over 100 ms windows and looks for 440 Hz alone, 440 and 660 Hz
together, and 880 Hz on each speaker, each well above the frequencies
around it (`sound::check_played`, `check_usb_played`). A test whose
sound is missing fails the boot.

## Limits

- 8 cards and 16 streams. A stream's buffer is four periods of at most
  64 KiB. virtio-sound periods are at most 16 KiB, four in flight.
- Formats: unsigned 8-bit and signed 16-bit PCM only. The server takes
  up to 8 channels.
- No volume control, no mixer controls of the cards, no channel maps
  (stereo is left, right).
- The server resamples by sample and hold. That is fine for tones and
  speech, but music at a far rate has audible aliasing.
- The virtio-sound event queue (jack and period notifications) is not
  driven.
- USB Audio: playback only (issue #610) and 16-bit PCM. It needs a host
  controller with isochronous transfers: the xHCI, EHCI, UHCI and OHCI
  have them, the DWC2 does not (issue #608).
- The capture test hears only silence: QEMU's `wav` backend has no input
  (issue #609).
