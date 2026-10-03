# Terminals

A terminal is a directory with two files, the Plan 9 way: `data` is the
terminal itself, what a program reads and writes, and `ctl` is its state as
text, read to see it and written to change it. Nothing about a terminal needs
`ioctl`.

```
/dev/console/data        the hardware console (serial and framebuffer)
/dev/console/ctl
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
```

The four flag lines and `speed` are the termios fields with Linux's bit values
(`toolchain/newlib/libgloss/myos/termios.h`), `cc` the 32 control characters
(`VINTR` first) as two hex digits each, `winsize` rows and columns. A write
takes the same lines back, and a line changes only what it names: `lflag 0`
puts the terminal in raw mode and leaves the rest alone, `cc 03` sets `VINTR`
only. Numbers are `0x` hex or decimal. Two more lines are commands:

| Line | Effect |
|------|--------|
| `ctty` | the terminal becomes the writer's controlling terminal (what `TIOCSCTTY` does) |
| `flush [in\|out\|both]` | discard the pending input and/or output; `both` when nothing is given |

A write applies all its lines or none: an unknown word, a bad number or a
line with extra words fails it with nothing changed. Lines must be whole
within one write. The console accepts `winsize` and ignores it (it is the
size of the screen), and has no output buffer to flush.

```
$ echo 'lflag 0' > $(readlink /proc/self/tty)/ctl     # raw mode
$ echo 'winsize 50 132' > /dev/pts/0/ctl
```

## Ptys

Opening `/dev/pts/clone` allocates a pair and returns the master fd; the
master's name in `/proc/self/fd` tells its index, so there is no `TIOCGPTN`.
The slave is `/dev/pts/N/data`; opening it does not make it the controlling
terminal, a session claims it with `ctty` on the pair's `ctl` (what `forkpty`
does for its child). The pair's termios, window size and session belong to
the pair: both ends see them. When the last fd on both ends is closed the
directory disappears. Closing the last master fd hangs up the session
(`SIGHUP`, then `EIO` on the slave), closing the last slave fd makes the
master's reads report `EIO` once drained ([`kernel/src/pty.rs`](../kernel/src/pty.rs)).

## Transition

The termios, window size and pty ioctls (`TCGETS`, `TCSETS`, `TCFLSH`,
`TIOCGWINSZ`, `TIOCSWINSZ`, `TIOCSCTTY`, `TIOCGPTN`, `TIOCSPTLCK`) and the old
names `/dev/ptmx` and `/dev/pts/N` (the slave) still work while libc moves to
the files; both go in the next steps, and the native `ioctl` syscall with
them. The Linux layer keeps speaking the Linux ioctl numbers for Alpine
binaries.
