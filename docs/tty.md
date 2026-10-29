# Terminals

A terminal in myos is a directory with two files, the Plan 9 way. `data`
is the terminal itself: what a program reads and writes. `ctl` is its
state as text: read it to see the state, write it to change it. myos has
no `ioctl`. Nothing about a terminal, or about any other device, goes
through one. This page covers the terminals, the text screens, the raw
keyboard and pointer files, and how libc and the Linux layer map the
usual tty calls onto them.

```
/dev/console/data        the console: the kernel's messages, the boot's, the daemons'
/dev/console/ctl
/dev/console/kbd         the keyboard's presses and releases, for a program that takes it
/dev/console/mouse       the pointer's motion and buttons, likewise
/dev/tty1/data           a terminal on text screen 1 of the framebuffer (tty1…tty9)
/dev/tty1/ctl
/dev/ttyS0/data          the serial port's terminal
/dev/ttyS0/ctl
/dev/pts/clone           open: allocates a pty pair, returns its master fd
/dev/pts/N/master        the master end, what clone returned (never opened by name)
/dev/pts/N/data          the slave, the terminal a session runs on
/dev/pts/N/ctl
/dev/tty                 POSIX: the calling process's controlling terminal (its data)
/proc/self/fd/N          symlink to what fd N is open on, /dev/pts/3/data for a terminal
/proc/self/tty           symlink to the controlling terminal's directory, /dev/pts/3
/proc/self/exe           symlink to the program the process runs (the file its exec loaded)
```

## Screens and logins

The framebuffer has ten text screens. One is shown at a time.

- Screen 0 is the console's. It shows the kernel's messages, the boot's
  status lines, and what processes write to `/dev/console/data` (the
  boot script `/etc/rc`, a daemon's `syslog`).
- Screens 1 to 9 are the terminals `/dev/tty1` to `/dev/tty9`.

Every screen keeps its text while another is shown, and is drawn whole
when it comes back. Text written to a screen that is not shown costs no
drawing.

To switch screens:

- `Alt+1` to `Alt+9` (the left Alt and the number row) show tty1 to
  tty9, and `Alt+0` shows the console. The console takes the key, so the
  shell never sees it. oksh's `Alt+digit` repeat count is lost with it.
- `screen N` written to `/dev/console/ctl` does the same. The ctl's
  `screen` line says which screen is shown.

The keyboard types at the shown screen's terminal: the console's on
screen 0, `ttyN` on screen N.

The serial port is `/dev/ttyS0`. The kernel's own messages always go to
it as well as to screen 0, so a crash shows on a headless machine. What
processes write to the console also goes to the serial port, until
`ttyS0` is first opened (by its getty, at the end of init's boot). So the
serial port shows the whole boot, and then becomes a terminal like the
others. Its input then goes to `ttyS0` instead of the console.

init starts a getty on `ttyS0` and on `tty1` to `ttyN`. It starts each
again when its session ends. Once they run, init shows tty1's screen:
the local login is on tty1, and the boot's lines are one `Alt+0` away.
`N` comes from `/data/etc/ttys` when the data partition has one, else
from the image's `/etc/ttys` (4). The file holds a number from 0 to 9 on
a line of its own.

```
$ echo 2 > /data/etc/ttys        # tty1 and tty2 at the next boot
```

### Programs that take the screen

A program can hold the screen (`/dev/fb` in `graphics` mode, an X
server). It then has the screen to itself: the text screens still take
text, but nothing is drawn. While a program holds the keyboard
(`/dev/console/kbd`), `Alt+N` is one more key for it, not a switch. When
it gives the screen back, the shown screen is drawn again.

### The ttys are ptys

The ttys are pty pairs whose master the kernel holds (see "Ptys" below).
Their termios, `ctty`, `^C` and window size work as a pty's, and
`/proc/self/tty` names them (`/dev/tty3`, `/dev/ttyS0`).

- A screen's window size is the screen's. `ttyS0` has the screen's size
  too, until a `winsize` line sets its own (the serial line has no size).
- They are `dev.tty` in the policy, like the ptys. Each belongs to its
  session's user. A user opens their own terminals, not another user's
  or one that no session holds (`docs/security.md`).
- A tty hangs up when a session claims it and when that session ends.
  The fds opened on its `data` before then read and write `EIO` from
  then on.

### `/dev/tty` and finding a terminal

`/dev/tty` stays a plain file, because POSIX names it and ported programs
open it as such. Its control file is `$(readlink /proc/self/tty)/ctl`.
A program that holds only an fd finds the terminal's directory through
`/proc/self/fd/N`. For a terminal the link's target ends in `/data`; a
pipe end reads `pipe:[N]`.

## The control file

```
$ cat /dev/console/ctl
iflag 0x100
oflag 0x5
cflag 0xb0
lflag 0x23b
cc 03 00 7f 00 04 00 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
speed 0 0
winsize 100 160
keymap /lib/kbd/ch.map
screen 1
```

| Line | Meaning |
|------|---------|
| `iflag`, `oflag`, `cflag`, `lflag`, `speed` | the termios fields, with Linux's bit values (`toolchain/newlib/libgloss/myos/termios.h`) |
| `cc` | the 32 control characters, `VINTR` first, two hex digits each |
| `winsize` | rows and columns |
| `keymap` | console only: the file its keyboard map was loaded from, or `none` |
| `screen` | console only: the text screen shown (above) |

A write takes the same lines back. A line changes only what it names:

- `lflag 0` puts the terminal in raw mode and leaves the rest alone;
- `cc 03` sets `VINTR` only;
- `screen 0` shows the console.

Numbers are `0x` hex or decimal. Three more lines are commands:

| Line | Effect |
|------|--------|
| `ctty` | the terminal becomes the writer's controlling terminal (what `TIOCSCTTY` does) |
| `flush [in\|out\|both]` | discard the pending input and/or output; `both` when nothing is given |
| `keymap PATH` | the console loads the keyboard map in that file (`docs/keymap.md`); a pty refuses it |

A pty refuses `screen` too.

A write applies all its lines or none. An unknown word, a bad number, a
line with extra words, or a keymap that cannot be loaded fails the write
with nothing changed. Lines must be whole within one write. The console
accepts `winsize` and ignores it, since its size is the screen's. It has
no output buffer to flush.

```
$ echo 'lflag 0' > $(readlink /proc/self/tty)/ctl     # raw mode
$ echo 'winsize 50 132' > /dev/pts/0/ctl
```

## The screen

The console's framebuffer copy (`modules/console/src/fb.rs`) draws an
8x8 font. It understands the VT100/xterm escapes that full-screen
programs use:

- cursor movement, erasing, scroll regions, the cursor's visibility;
- colors: the 8 ANSI ones (`30`–`37`, `40`–`47`, bright `90`–`97`),
  xterm's 256 (`38;5;N`, `48;5;N`) and RGB (`38;2;R;G;B`,
  `48;2;R;G;B`).

Text is UTF-8. A character takes one cell, a wide one (East Asian, emoji)
two, and a combining mark none, as a terminal program counts them.
Besides ASCII, the font draws:

- box drawing (U+2500–257F: light, heavy, double and rounded lines);
- block elements (U+2580–259F: halves, eighths, shades, quadrants);
- braille patterns (U+2800–28FF, which programs such as `btm` draw
  graphs with);
- a few symbols (arrows, triangles, `•`, `…`, `°`).

Any other character is a `?` in its cells, and so are bytes that are not
UTF-8. The serial port gets the bytes as written.

## The raw keyboard

`/dev/console/kbd` is the local keyboard as key events. That is PS/2 on
x86_64, virtio-input on aarch64 and riscv64, and the USB keyboards on
every arch. It is for programs that need more than the characters a
terminal gives: a game, or a graphical session drawing on `/dev/fb`
(`docs/fb.md`). One event per line:

```
d 42          Left Shift pressed
d 30 A        A pressed (with Shift held, the keymap gives "A")
u 30          A released
u 42
```

- `d <code>[ <char>]` is a press, `u <code>` a release.
- The code is Linux's `KEY_*` number (`linux/input-event-codes.h`) on
  every arch: Left Ctrl 29, Enter 28, the arrows 103/105/106/108.
- The character is what the loaded keymap (`docs/keymap.md`) gives the
  key with the Shift and AltGr held now, as UTF-8. It is left out for
  space, for control characters (Enter, Backspace, Tab, Esc), for keys
  the keymap does not map, and for every key while no keymap is loaded.
- There is no autorepeat: a held key is one `d` and, later, one `u`.

Opening the file takes the keyboard. Until its last fd closes (or its
holder exits), no terminal gets keys from it and `Alt+N` does not switch
screens. The serial port's terminal is not affected. One program holds
the file at a time; another open fails.

- Reads return whole lines. A read waits for the next event, and `poll`
  reports `POLLIN` while one is queued.
- The queue holds 128 events and drops new ones while full.
- The keyboard interrupts on input where the platform can route it (IRQ 1
  through the I/O APIC, the virtio-input device's interrupt). Otherwise
  it is polled every 10 ms while someone waits.
- The virtio-input driver keeps 64 event buffers posted, the device's
  whole queue, because QEMU drops a key event when the guest has none
  posted (issue #650, History below).

The file exists when the console module found a keyboard
([`modules/console/src/kbdev.rs`](../modules/console/src/kbdev.rs)), or
from the first USB keyboard on.

### Keyboards of other modules

A keyboard another module drives (`usb_hid`'s USB keyboards,
`docs/usb.md`) hands the console its presses and releases through the
`keyboard` service
([`modules/console/src/keyboards.rs`](../modules/console/src/keyboards.rs)),
in the same `KEY_*` codes. They go where the console's own keyboard's go:
this file, the shown screen's terminal, `Alt+N`. Every keyboard shares
the modifiers and Caps Lock.

A USB keyboard does not repeat a held key, so the console repeats it, for
the terminals only: 500 ms after the press, then every 33 ms, until the
key is released or another is pressed. A USB keyboard's Caps Lock LED
follows the console's Caps Lock.

### Input without a reader

Keyboard and serial input goes through the line discipline as it
arrives, whether or not anything reads the terminal. The `console-input`
kernel thread does this
([`kernel/src/input.rs`](../kernel/src/input.rs)). So `^C` interrupts a
job that never reads the terminal, such as `sha512sum /dev/zero`, and
typed-ahead input is echoed at once.

A read of the console (`/dev/console/data`, `/dev/tty`) waits as long as
it takes; a shell at its prompt waits until the next key. It does not
hold the filesystem tree meanwhile, so a rename, unlink or new file
elsewhere does not wait for that key (devfs's `waits`,
`kernel/src/fs/vfs.rs`).

## The pointer

`/dev/console/mouse` is the pointer: what every mouse and tablet does. It
is for the same programs as the raw keyboard; Xfbdev's `kdrive/myos`
reads it (`packages/tinyx`). The console module serves the file and
publishes the `pointer` service
([`PointerOps`](../modules/abi/src/lib.rs)). These feed it, all into the
one file:

- a pointing device's driver in another module, today `usb_hid` (USB HID
  mice and tablets, `docs/usb.md`);
- the console's own: the PS/2 mouse on x86_64, and virtio-input mice and
  tablets on the `virt` boards.

One event per line:

```
m 10 -5       moved 10 right and 5 up
a 16384 8192  a tablet's position: the middle of the screen, a quarter down
w 1           the wheel, one step up (away from the user)
d 272         left button pressed
u 272         released
```

| Event | Meaning |
|-------|---------|
| `m <dx> <dy>` | relative motion, right and down positive |
| `a <x> <y>` | an absolute position, both axes from 0 (left, top) to 32767 whatever the device's own range; the reader scales it to its screen |
| `w <dz>` | the vertical wheel |
| `d <code>`, `u <code>` | a button pressed or released, as Linux's `BTN_*` code: left 272, right 273, middle 274, side 275, extra 276 |

Motion not read yet is merged, so a slow reader gets fewer, larger steps
rather than lost ones. Two `m` lines in a row become one, as do two `w`,
and a newer `a` replaces the one before it. A button held when its mouse
is pulled out is released (`u`).

The file is there whether or not a pointer is plugged in, since a USB
mouse may come later. Opening it takes the pointer: one program holds it
at a time, another open fails, and nothing is queued while nobody holds
it. Reads return whole lines. A read waits for the next event, and `poll`
reports `POLLIN` while one is queued (every event wakes the waiters). The
queue holds 128 events and drops new ones while full.

### The PS/2 mouse

The PS/2 mouse is on the keyboard controller's second (aux) port. At boot
the console sets it to its defaults and asks for the IntelliMouse wheel:
the packets are then four bytes, three without it. The console reads the
packets along with the keyboard's bytes. When a mouse answered, it
prints `[ OK ] mouse (ps2, wheel)` on the console. Its buttons are
left, right and middle.

The two ports share the controller's one output buffer, so a mouse byte
nobody takes holds back the keyboard's. The console therefore uses the
port only in two cases:

- its IRQ 12 is routed, as the keyboard's IRQ 1 is (both show in
  `/proc/interrupts`, as `ps2 mouse` and `ps2 keyboard`);
- the whole controller is polled because neither IRQ is routed. Then
  `/dev/console/mouse`'s readers and the console's input thread read
  the mouse every 10 ms
  ([`modules/console/src/ps2.rs`](../modules/console/src/ps2.rs)).

### virtio-input

On the `virt` boards, a virtio-input device whose config space lists
relative axes (`EV_REL`) is a mouse. One with absolute axes (`EV_ABS`) is
a tablet, whose `ABS_X`/`ABS_Y` ranges scale its position. The first
other device is the keyboard.

- Up to two pointers. Each shows `[ OK ] mouse (virtio)` or
  `tablet (virtio)` on the console, and `virtio-pointer` in
  `/proc/interrupts`.
- A frame's axes go to the file at its `SYN_REPORT`. Buttons
  (`BTN_LEFT` to `BTN_EXTRA`) go at once
  ([`modules/console/src/virtio_input.rs`](../modules/console/src/virtio_input.rs)).

## Ptys

Opening `/dev/pts/clone` allocates a pair and returns the master fd. The
master's name in `/proc/self/fd` tells its index, so there is no
`TIOCGPTN`. The slave is `/dev/pts/N/data`.

Opening the slave does not make it the controlling terminal. A session
leader claims it with `ctty` on the pair's `ctl` (`TIOCSCTTY`; this is
what `forkpty`'s child, dropbear's login and st do). A session leader is
a process that called `setsid`, which also drops the console as its
terminal.

- From then on the pair is the controlling terminal of every process in
  that session: their `/dev/tty` and `/proc/self/tty`.
- This lasts until the leader exits. The pair is then nobody's. A later
  session leader with the same pid (a reused task slot) neither holds it
  nor keeps the next claim out.
- A claim by a process that leads no session is ignored, and so is a
  claim of a pair that another live session holds.

The pair's termios, window size and session belong to the pair: both ends
see them. When the last fd on both ends is closed, the directory
disappears.

- Closing the last master fd hangs up the session: `SIGHUP` to the
  session leader's process group, then `EIO` on the slave.
- Closing the last slave fd makes the master's reads report `EIO` once
  drained ([`kernel/src/pty.rs`](../kernel/src/pty.rs)).

There is no fixed number of pairs. Each takes a few KiB of kernel heap
and holds at least one fd, so the fd limits bound them. A freed index is
reused first.

The kernel's terminals (`ttyS0`, `tty1`…`tty9`) are pairs too, made at
boot, whose master the kernel holds for good. Their output goes to the
serial port or their screen instead of a ring a master reads. Their input
comes from the serial port or the keyboard. They are never hung up.

## libc

Nothing in libgloss or the Rust `libc` crate issues an ioctl syscall.
libgloss finds the terminal behind an fd through its `/proc/self/fd` link
(`toolchain/newlib/libgloss/myos/ttyctl.c`):

| Call | How |
|------|-----|
| `tcgetattr` | reads the ctl |
| `tcsetattr` | writes the termios lines |
| `tcflush` | writes a `flush` line |
| `isatty` | asks whether the link ends in `/data` (or `/master`) |
| `ttyname` | returns the link's target |
| `openpty` | opens `clone` and reads the master's link for the pair's directory |

The `ioctl()` function is a shim for the requests ported programs make
(`TCGETS`, `TCSETS`, `TCFLSH`, `TIOCGWINSZ`, `TIOCSWINSZ`, `TIOCSCTTY`,
`TIOCGPTN`, `TIOCSPTLCK`), served the same way. Anything else is
`ENOTTY`. The Rust `libc` crate (`ports/crates/libc/myos.rs`) does the
same for `isatty` and `ioctl`.

## The Linux layer

Alpine binaries speak the Linux tty ioctls (`TCGETS`, `TCSETS`, `TCFLSH`,
`TIOCGWINSZ`, `TIOCSWINSZ`, `TIOCSCTTY`, `TIOCGPTN`, `TIOCSPTLCK`). The
Linux module serves them from the same ctl text, which the kernel hands
it for an fd (`KernelApi::tty_ctl_read` and `tty_ctl_write`,
`modules/linux/src/tty.rs`). It converts to and from Linux's
`struct termios`: the 36-byte kernel layout with `c_line` and 19 control
characters. It maps musl's `/dev/ptmx` and `/dev/pts/N` onto `clone` and
`data`, and `TIOCGPTN` reads the master's name (`KernelApi::fd_path`).

## Devices

There is no `ioctl` syscall. A module's character device is a directory
like a terminal:

- `/dev/net0/data` is the NIC (Ethernet frames);
- `/dev/net0/ctl` is its state as text (`mac 52:54:00:12:34:56`,
  `irq on`);
- `/dev/netd/data` is the netfs channel to `netd`.

A device says when its `data` is ready through `poll`. So `netd` sleeps
in `poll` on the NIC and its channel instead of in a blocking request
(`myos_abi::ModuleChrOps`, `docs/scheduler.md`).

## History

- The virtio-input driver used to post eight event buffers. That gave a
  burst of keys 40 ms of slack (QEMU's `sendkey` types a press and a
  release 10 ms apart), and a reader woken that late under TCG missed
  keys. It now posts 64, the device's whole queue (issue #650).
