# Terminals

A terminal is a directory with two files, the Plan 9 way: `data` is the
terminal itself, what a program reads and writes, and `ctl` is its state as
text, read to see it and written to change it. myos has no `ioctl`: nothing
about a terminal, or any device, goes through one.

```
/dev/console/data        the hardware console (serial and framebuffer)
/dev/console/ctl
/dev/console/kbd         the keyboard's presses and releases, for a program that takes it
/dev/pts/clone           open: allocates a pty pair, returns its master fd
/dev/pts/N/master        the master end, what clone returned (never opened by name)
/dev/pts/N/data          the slave, the terminal a session runs on
/dev/pts/N/ctl
/dev/tty                 POSIX: the calling process's controlling terminal (its data)
/proc/self/fd/N          symlink to what fd N is open on, /dev/pts/3/data for a terminal
/proc/self/tty           symlink to the controlling terminal's directory, /dev/pts/3
```

`/dev/tty` stays a plain file because POSIX names it and ported programs open
it as such. Its control file is `$(readlink /proc/self/tty)/ctl`. A program
holding only an fd finds the terminal's directory through `/proc/self/fd/N`,
whose target ends in `/data` for a terminal; a pipe end reads `pipe:[N]`.

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
mirror on
```

The four flag lines and `speed` are the termios fields with Linux's bit values
(`toolchain/newlib/libgloss/myos/termios.h`), `cc` the 32 control characters
(`VINTR` first) as two hex digits each, `winsize` rows and columns. The console
has two more lines: `keymap`, the file its keyboard map was loaded from, or
`none`; `mirror`, whether its output goes to the screen as well as to the
serial port (`on`, the default) or to the serial port only (`off`: scrolling
a big framebuffer is slow under emulation, so the boot tests' runner turns it
off while they run). A write takes the same lines back, and a line changes
only what it names: `lflag 0` puts the terminal in raw mode and leaves the
rest alone, `cc 03` sets `VINTR` only, `mirror off` stops the screen copy.
Numbers are `0x` hex or decimal. Three more lines are commands:

| Line | Effect |
|------|--------|
| `ctty` | the terminal becomes the writer's controlling terminal (what `TIOCSCTTY` does) |
| `flush [in\|out\|both]` | discard the pending input and/or output; `both` when nothing is given |
| `keymap PATH` | the console loads the keyboard map in that file (`docs/keymap.md`); a pty refuses it |

A pty refuses `mirror` too.

A write applies all its lines or none: an unknown word, a bad number, a line
with extra words or a keymap that cannot be loaded fails it with nothing
changed. Lines must be whole within one write. The console accepts `winsize`
and ignores it (it is the size of the screen), and has no output buffer to
flush.

```
$ echo 'lflag 0' > $(readlink /proc/self/tty)/ctl     # raw mode
$ echo 'winsize 50 132' > /dev/pts/0/ctl
```

## The screen

The console's framebuffer copy (`modules/console/src/fb.rs`) draws an 8x8
font and understands the VT100/xterm escapes full-screen programs use:
cursor movement, erasing, scroll regions, the cursor's visibility, and
colors, the 8 ANSI ones (`30`–`37`, `40`–`47`, bright `90`–`97`), xterm's
256 (`38;5;N`, `48;5;N`) and RGB (`38;2;R;G;B`, `48;2;R;G;B`).

Text is UTF-8. A character takes one cell, a wide one (East Asian, emoji)
two and a combining mark none, as a terminal program counts them. Besides
ASCII, the font draws box drawing (U+2500–257F: light, heavy, double and
rounded lines), block elements (U+2580–259F: halves, eighths, shades,
quadrants), braille patterns (U+2800–28FF, which programs such as `btm`
draw graphs with) and a few symbols (arrows, triangles, `•`, `…`, `°`). Any
other character is a `?` in its cells, as are bytes that are not UTF-8.
The serial port gets the bytes as written.

## The raw keyboard

`/dev/console/kbd` is the local keyboard (PS/2 on x86_64, virtio-input on
aarch64 and riscv64) as key events, for programs that need more than the
characters a terminal gives: a game, a graphical session drawing on
`/dev/fb` (`docs/fb.md`). One event per line:

```
d 42          Left Shift pressed
d 30 A        A pressed (with Shift held, the keymap gives "A")
u 30          A released
u 42
```

`d <code>[ <char>]` is a press, `u <code>` a release. The code is Linux's
`KEY_*` number (`linux/input-event-codes.h`) on every arch: Left Ctrl 29,
Enter 28, the arrows 103/105/106/108. The character is what the loaded keymap
(`docs/keymap.md`) gives the key with the Shift and AltGr held now, as UTF-8;
it is left out for space, control characters (Enter, Backspace, Tab, Esc),
keys the keymap does not map and every key while no keymap is loaded. There is
no autorepeat: a held key is one `d` and, later, one `u`.

Opening the file takes the keyboard: until its last fd closes (or its
holder exits) the console tty gets no keys from it. Serial input still
reaches the tty, so the serial console stays usable. One program holds the
file at a time; another open fails. Reads return whole lines; a read waits
for the next event, `poll` reports `POLLIN` while one is queued. The queue
holds 128 events and drops new ones while full. The keyboard interrupts
on input where the platform can route it (IRQ 1 through the I/O APIC, the
virtio-input device's interrupt); otherwise it is polled every 10 ms while
someone waits. The file exists when the
console module found a keyboard ([`modules/console/src/kbdev.rs`](../modules/console/src/kbdev.rs)).

A read of the console (`/dev/console/data`, `/dev/tty`) waits for as long as
it takes, a shell at its prompt until the next key, without holding the
filesystem tree: a rename, unlink or new file elsewhere does not wait for
that key (devfs's `waits`, `kernel/src/fs/vfs.rs`).

## Ptys

Opening `/dev/pts/clone` allocates a pair and returns the master fd; the
master's name in `/proc/self/fd` tells its index, so there is no `TIOCGPTN`.
The slave is `/dev/pts/N/data`; opening it does not make it the controlling
terminal: a session leader (a process that called `setsid`, which also
drops the console as its terminal) claims it with `ctty` on the pair's `ctl`
(`TIOCSCTTY`, what `forkpty`'s child, dropbear's login and st do). From then
on the pair is the controlling terminal of every process in that session,
its `/dev/tty` and `/proc/self/tty`, until the leader exits; a claim by a
process that leads no session, or of a pair another live session holds, is
ignored. The pair's termios, window size and session belong to
the pair: both ends see them. When the last fd on both ends is closed the
directory disappears. Closing the last master fd hangs up the session
(`SIGHUP` to the session leader's process group, then `EIO` on the slave),
closing the last slave fd makes the
master's reads report `EIO` once drained ([`kernel/src/pty.rs`](../kernel/src/pty.rs)).
There is no fixed number of pairs: each takes a few KiB of kernel heap and
holds at least one fd, so the fd limits bound them, and a freed index is
reused first.

## libc

Nothing in libgloss or the Rust `libc` crate issues an ioctl syscall. The
terminal behind an fd is found through its `/proc/self/fd` link
(`toolchain/newlib/libgloss/myos/ttyctl.c`): `tcgetattr` reads the ctl,
`tcsetattr` writes the termios lines, `tcflush` a `flush` line, `isatty` asks
whether the link ends in `/data` (or `/master`), `ttyname` returns the link's
target, `openpty` opens `clone` and reads the master's link for the pair's
directory. The `ioctl()` function is a shim for the requests ported programs
make (`TCGETS`, `TCSETS`, `TCFLSH`, `TIOCGWINSZ`, `TIOCSWINSZ`, `TIOCSCTTY`,
`TIOCGPTN`, `TIOCSPTLCK`), served the same way; anything else is `ENOTTY`.
The Rust `libc` crate (`ports/crates/libc/myos.rs`) does the same for
`isatty` and `ioctl`.

## The Linux layer

Alpine binaries speak the Linux tty ioctls (`TCGETS`, `TCSETS`, `TCFLSH`,
`TIOCGWINSZ`, `TIOCSWINSZ`, `TIOCSCTTY`, `TIOCGPTN`, `TIOCSPTLCK`). The Linux
module serves them from the same ctl text, which the kernel hands it for an fd
(`KernelApi::tty_ctl_read` and `tty_ctl_write`, `modules/linux/src/tty.rs`),
converting to and from Linux's `struct termios` (the 36-byte kernel layout
with `c_line` and 19 control characters). It maps musl's `/dev/ptmx` and
`/dev/pts/N` onto `clone` and `data`, and `TIOCGPTN` reads the master's name
(`KernelApi::fd_path`).

## Devices

There is no `ioctl` syscall. A module's character device is a directory like
a terminal: `/dev/net0/data` is the NIC (Ethernet frames), `/dev/net0/ctl` its
state as text (`mac 52:54:00:12:34:56`, `irq on`), `/dev/netd/data` the netfs
channel to `netd`. A device says when its `data` is ready through `poll`, so
`netd` sleeps in `poll` on the NIC and its channel instead of a blocking
request (`myos_abi::ModuleChrOps`, `docs/pci-acpi-smp.md`).
