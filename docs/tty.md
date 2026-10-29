# Terminals

A terminal in myos is a directory with two files, the Plan 9 way. `data`
is the terminal itself: what a program reads and writes. `ctl` is its
state as text: read it to see the state, write it to change it. myos has
no `ioctl`, for terminals or any other device. This page covers the
terminals, the text screens, the raw keyboard and pointer files, and how
libc and the Linux layer map the usual tty calls onto them.

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
/proc/self/exe           symlink to the program the process runs
```

## Screens and logins

The framebuffer has ten text screens, one shown at a time
(`modules/console`):

- screen 0 is the console's: the kernel's messages, the boot's status
  lines, and what processes write to `/dev/console/data` (`/etc/rc`, a
  daemon's `syslog`);
- screens 1 to 9 are the terminals `/dev/tty1` to `/dev/tty9`.

`Alt+1` to `Alt+9`
(left Alt, number row) show tty1 to tty9 and `Alt+0` the console; the
console takes the key, so the shell never sees it (oksh's `Alt+digit`
repeat count is lost). Writing `screen N` to `/dev/console/ctl` does the
same. The keyboard types at the shown screen's terminal.

The serial port is `/dev/ttyS0`. The kernel's messages always go to it as
well as to screen 0, so a crash shows on a headless machine. Console
output also goes there until `ttyS0` is first opened by its getty; the
serial port thus shows the whole boot, then becomes a terminal like the
others.

init starts a getty on `ttyS0` and on `tty1` to `ttyN` and shows tty1. `N` comes from `/data/etc/ttys`
when it exists, else the image's `/etc/ttys` (4): a number from 0 to 9.

```
$ echo 2 > /data/etc/ttys        # tty1 and tty2 at the next boot
```

A program that holds the screen (`/dev/fb` in `graphics` mode, an X
server) has it to itself: the text screens still take text but nothing is
drawn until it gives the screen back. While a program holds
`/dev/console/kbd`, `Alt+N` is one more key for it.

### The ttys are ptys

The ttys are pty pairs whose master the kernel holds ("Ptys" below), so
their termios, `ctty`, `^C` and window size work as a pty's.

- Their window size is the screen's (`ttyS0`'s until a `winsize` line).
- They are `dev.tty` in the policy: each belongs to its session's user,
  who opens only their own (`docs/security.md`).
- A tty hangs up when a session claims it and when that session ends; fds
  opened on it before then get `EIO`.

### `/dev/tty` and finding a terminal

`/dev/tty` stays a plain file, because POSIX names it and ported programs
open it. Its control file is `$(readlink /proc/self/tty)/ctl`. A program
holding only an fd finds the terminal through `/proc/self/fd/N`: for a
terminal the target ends in `/data`; a pipe reads `pipe:[N]`.

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
| `screen` | console only: the text screen shown |

A write takes the same lines back, and a line changes only what it names
(`lflag 0` is raw mode, `cc 03` sets `VINTR` only). Numbers are `0x` hex
or decimal. Three more lines are commands:

| Line | Effect |
|------|--------|
| `ctty` | the terminal becomes the writer's controlling terminal (`TIOCSCTTY`) |
| `flush [in\|out\|both]` | discard pending input and/or output; `both` by default |
| `keymap PATH` | the console loads that keyboard map (`docs/keymap.md`) |

A pty refuses `keymap` and `screen`; the console ignores `winsize`. A
write applies all its lines or none, and lines must be whole within one
write.

```
$ echo 'lflag 0' > $(readlink /proc/self/tty)/ctl     # raw mode
$ echo 'winsize 50 132' > /dev/pts/0/ctl
```

## The screen

The console draws an 8x8 font (`modules/console/src/fb.rs`) and
understands the VT100/xterm escapes full-screen programs use, with 8,
256 and RGB colors. Text is UTF-8 (wide characters take two cells).
Besides ASCII the font has box drawing, block elements, braille and a few
symbols; anything else is a `?`. The serial port gets the bytes as
written.

## The raw keyboard

`/dev/console/kbd` is the local keyboard as key events: PS/2 on x86_64,
virtio-input on aarch64 and riscv64, USB keyboards everywhere. It is for
a game or a graphical session on `/dev/fb` (`docs/fb.md`). One event per
line:

```
d 42          Left Shift pressed
d 30 A        A pressed (with Shift held, the keymap gives "A")
u 30          A released
u 42
```

- The code is Linux's `KEY_*` number (`linux/input-event-codes.h`) on
  every arch.
- The character is what the loaded keymap (`docs/keymap.md`) gives the
  key with the modifiers now held, as UTF-8. It is left out for space,
  control characters, unmapped keys, and while no keymap is loaded.
- There is no autorepeat.

Opening the file takes the keyboard: until its last fd closes, no
terminal gets keys and `Alt+N` does not switch (the serial port is not
affected). One program holds it at a time; another open fails. Reads
return whole lines, `poll` works, and the queue drops new events past
128. The file exists once a keyboard is found
(`modules/console/src/kbdev.rs`).

Other modules' keyboards (`usb_hid`, `docs/usb.md`) feed the console
through its `keyboard` service (`modules/console/src/keyboards.rs`), in
the same codes, and share its modifiers and Caps Lock. A USB keyboard
does not repeat, so the console repeats held keys for the terminals (500
ms, then every 33 ms).

Keyboard and serial input goes through the line discipline as it
arrives, whether or not anything reads the terminal (the `console-input`
thread, `kernel/src/input.rs`). So `^C` interrupts a job that never reads
the terminal, and typed-ahead input echoes at once. A blocked read of the
console does not hold the filesystem tree (devfs's `waits`,
`kernel/src/fs/vfs.rs`).

## The pointer

`/dev/console/mouse` is every mouse and tablet merged into one file, for
the same programs as the raw keyboard; Xfbdev's `kdrive/myos` reads it
(`packages/tinyx`). The console feeds it from the PS/2 mouse (x86_64,
with the wheel), virtio-input mice and tablets (`virt` boards,
`modules/console/src/virtio_input.rs`), and other modules through its
`pointer` service (`PointerOps`, `modules/abi/src/lib.rs`; today
`usb_hid`). One event per line:

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
| `a <x> <y>` | an absolute position, 0 to 32767 on both axes whatever the device; the reader scales it |
| `w <dz>` | the vertical wheel |
| `d <code>`, `u <code>` | a button, as Linux's `BTN_*` code: left 272, right 273, middle 274, side 275, extra 276 |

Unread motion is merged, so a slow reader gets fewer, larger steps rather
than lost ones. The file is always there, since a USB mouse may come
later. Opening it takes the pointer, as with the keyboard; nothing is
queued while nobody holds it.

The PS/2 mouse shares the keyboard controller's output buffer, so the
console uses it only when its IRQ 12 is routed or the whole controller is
polled (`modules/console/src/ps2.rs`); otherwise an unread mouse byte
would block the keyboard.

## Ptys

Opening `/dev/pts/clone` allocates a pair and returns the master fd; its
name in `/proc/self/fd` gives the index. The slave is `/dev/pts/N/data`
(`kernel/src/pty.rs`).

A session leader (after `setsid`) makes the slave its controlling
terminal with `ctty` on the pair's `ctl`, as `forkpty`'s child, dropbear
and st do; opening the slave does not. The claim lasts until the leader
exits, and is ignored from a non-leader or for a pair another live
session holds.

The termios, window size and session belong to the pair. When the last fd
on both ends closes, the directory disappears. Closing the last master fd
hangs up the session (`SIGHUP`, then `EIO` on the slave); closing the
last slave fd makes the master read `EIO` once drained.

There is no fixed number of pairs: each takes a few KiB of heap and at
least one fd, so the fd limits bound them. The kernel's terminals
(`ttyS0`, `tty1`…`tty9`) are pairs made at boot whose master the kernel
holds for good, wired to the serial port or a screen, and never hung up.

## libc

Nothing in libgloss or the Rust `libc` crate issues an ioctl syscall.
libgloss finds the terminal behind an fd through its `/proc/self/fd` link
(`toolchain/newlib/libgloss/myos/ttyctl.c`): `tcgetattr` reads the ctl,
`tcsetattr` and `tcflush` write it, `isatty` checks that the link ends in
`/data` (or `/master`), `ttyname` returns it, and `openpty` opens
`clone`. The `ioctl()` function is a shim for the requests ported
programs make (`TCGETS`, `TCSETS`, `TCFLSH`, `TIOCGWINSZ`, `TIOCSWINSZ`,
`TIOCSCTTY`, `TIOCGPTN`, `TIOCSPTLCK`); anything else is `ENOTTY`. The
Rust `libc` crate does the same (`ports/crates/libc/myos.rs`).

## The Linux layer

The Linux module serves the same tty ioctls to Alpine binaries from the
ctl text, which the kernel hands it for an fd (`KernelApi::tty_ctl_read`
and `tty_ctl_write`, `modules/linux/src/tty.rs`), converting to Linux's
`struct termios`. It maps musl's `/dev/ptmx` and `/dev/pts/N` onto
`clone` and `data`.

## Devices

Other character devices follow the same pattern (`/dev/net0/data` and
`ctl`, `/dev/netd/data`) and report readiness through `poll`
(`myos_abi::ModuleChrOps`, `docs/scheduler.md`).

## Testing

`user/tests/kernel.sh` covers the terminal files and the kernel's
terminals; the keyboard, pointer and screen smokes are in `user/c`
(`kbd_smoke.c`, `mouse_smoke.c`, `fb_smoke.c`, run from `user/c/test.sh`).

## History

- The virtio-input driver posts 64 event buffers, the device's whole
  queue, because QEMU drops key events when the guest has none posted;
  with eight, a reader woken late under TCG missed keys (issue #650).
