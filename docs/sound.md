# Sound

Three layers: the card drivers (`virtio_sound`, `usb_audio`) drive the
hardware; the `sound` module, the core, gives each of their PCM streams a
buffer and a directory under `/dev/sound`; the sound server, `sndd`, owns
those directories and mixes what every program plays into them. Programs
talk to the server over a local socket, never to `/dev/sound` (a stream's
`data` is held by one program at a time). `play` and `rec`
(`user/c/play.c`, `/bin/etc/`) are its clients.

| Piece | What |
|-------|------|
| `modules/sound` | The core: the cards and their streams, `/dev/sound/N/{ctl,data}`, each stream's buffer, the `sound` service (`SoundOps`) the card drivers register with |
| `modules/virtio_sound` | virtio-sound cards (virtio 1.2, device 25; PCI or MMIO): every playback and capture stream they have |
| `modules/usb_audio` | USB Audio Class 1 speakers: one playback stream over the streaming interface's isochronous OUT endpoint (`docs/usb.md`) |
| `user/sndd` | The sound server: the `sndd` socket, mixing, rate, channel and format conversion |
| `user/c/play.c` | `play` (a tone or a WAV file) and `rec` (into a WAV file), the server's clients |

## `/dev/sound`

A card driver registers its card with its streams (`card_register`: a
name, and per stream its direction, channel range, formats and rates);
the streams are numbered across the cards in the order they came, and
stream `N` is `/dev/sound/N`:

- `ctl` reads as lines: `card NAME` (`virtio-sound 00:04.0`, `usb-audio
  46f4:0002`), `direction playback|capture`, `channels MIN-MAX`, `formats
  u8 s16`, `rates ...` (of 5512 to 384000 Hz, the virtio-sound list),
  `state stopped` or `state running RATE CHANNELS FORMAT period BYTES
  buffer BYTES`, `underruns N`, `overruns N`. It takes `start RATE
  CHANNELS FORMAT [PERIOD_FRAMES]` (a 20 ms period without one) and
  `stop`.
- `data` is the PCM: interleaved frames, unsigned 8-bit or signed 16-bit
  little endian. One program holds it at a time; its last close stops
  the stream. A playback stream's writes take the whole frames that fit
  in its buffer (four periods), and `poll` says writable while a period
  fits; a capture stream's reads return what was recorded, waiting while
  it runs and nothing is there.

The driver takes what a playback stream plays a period at a time
(`playback_take`), silence for what the buffer lacks (an underrun), and
hands over what a capture stream recorded (`capture_put`), the oldest
dropped when nobody reads it (an overrun). A card that goes (a speaker
pulled out) takes its streams with it (`card_remove`): their files are
gone, a held `data` fails.

The virtio-sound driver is its own (the `virtio-drivers` crate's
busy-waits for as long as the sound lasts and has no capture): the
crate's transport and split queues, the control queue for `PCM_INFO`,
`SET_PARAMS`, `PREPARE`, `START`, `STOP` and `RELEASE`, four periods in
flight on the tx or rx queue per running stream. A card's kernel thread
refills or drains each period as it comes back, woken by the device's
interrupt.

## The sound server

`sndd` is started by init (in the `sndd` domain, `docs/security.md`) and
listens on the local socket `sndd` (`/net/unix`, `docs/sockets-unix.md`).
A connection starts with a line, answered `ok` or `error WHY`:

- `play RATE CHANNELS FORMAT [STREAM]`: the program then writes PCM in
  that format until it closes, into `STREAM` (the first playback stream
  without one). Everything playing on a stream is mixed: each program's
  frames converted to the stream's rate (sample and hold, a 32.32 fixed
  point step), channels (mono to every channel, any to mono by
  averaging, else the channels both have) and format, summed and clipped.
- `record RATE CHANNELS FORMAT [STREAM]`: the server writes what the
  capture stream (the first one without `STREAM`) records, converted,
  until the program closes; what it does not read in time is dropped.
- `devices`: one line per stream, `N playback|capture CARD`.

A stream runs while a program uses it, at 48 kHz stereo `s16` when the
card can (else the closest it has), and stops half a second after the
last one went. A program buffers up to 64 KiB in the server; past that
its writes wait in the socket, so `play` keeps pace with the card.

```sh
play -t 440 -d 2                 # a 440 Hz tone for two seconds
play -r 22050 -c 1 -f u8 -t 440,660 -d 1   # two tones, mono 8-bit
play -D 3 file.wav               # a WAV file (u8 or s16 PCM) on stream 3
rec -r 16000 -c 1 -d 5 out.wav   # five seconds from the first capture stream
rec -d 1 -n                      # just say how many frames came
play -l                          # the server's `devices`
```

## In QEMU

Every board with PCI has two sound devices (`src/sound.rs`): a
virtio-sound card and a `usb-audio` speaker on the xHCI's root port 5;
the Raspberry Pi 3 has neither. In a window (`cargo run`) both play where
`MYOS_AUDIODEV` says, a QEMU `-audiodev` driver (`pa`, `pipewire`,
`alsa`, `coreaudio`, `sdl`, ...; `none` without it):

```sh
MYOS_AUDIODEV=pa cargo run -- uefi
```

A test boot has two virtio-sound cards and three USB speakers (the
xHCI's, and one behind a hub on a UHCI and on an OHCI companion,
`docs/usb.md`). The first card (playback only) and the speakers play
into WAV files of QEMU's `wav` backend at 48 kHz stereo
(`target/boot-test-<name>-sound.wav`, and `.sound.usb.wav`,
`.sound.uhci.wav`, `.sound.ohci.wav`); the second card records QEMU's
`none` input, silence.
`user/sndd/test.sh` (tag `sound`) checks the streams and the server's
list (`sound_devices`), plays a tone (`sound_play`: the stream runs at
48 kHz stereo while it plays and stops after), two programs at once at
different rates and formats (`sound_mix`), records two seconds at 16 kHz
mono (`sound_rec`: about the right number of frames, up to twice as
many on a loaded host, whose clock QEMU's input follows while `rec`'s is
the guest's; all silent) and
plays through every speaker (`sound_usb`). Once QEMU is gone the launcher
reads the WAV files and looks, with a Goertzel filter over 100 ms
windows, for 440 Hz alone, 440 and 660 Hz together, and 880 Hz on each
speaker, each well above the frequencies around it (`sound::check_played`,
`check_usb_played`); a test whose sound is missing fails the boot.

## Limits

- 8 cards, 16 streams, four periods of at most 64 KiB per stream's
  buffer; virtio-sound periods of at most 16 KiB, four in flight.
- Formats: unsigned 8-bit and signed 16-bit PCM only; up to 8 channels in
  the server. No volume control, no mixer controls of the cards, no
  channel maps (stereo is left, right).
- The server's resampling is sample and hold: fine for tones and speech,
  audible aliasing on music at a far rate.
- The virtio-sound event queue (jack and period notifications) is not
  driven.
- USB Audio: playback only (issue #610), 16-bit PCM, on the xHCI only
  (issue #608).
- The capture test hears silence only: QEMU's `wav` backend has no input
  (issue #609).
