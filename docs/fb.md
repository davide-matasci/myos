# Framebuffer: `/dev/fb`

`/dev/fb` is the screen, for programs that draw on it themselves. Like
`/net`, it is a directory of files rather than a device with ioctls. The
console module serves it (`modules/console/src/fbdev.rs`), since it already
owns the screen. A program on the screen reads its input from
`/dev/console/kbd` and `/dev/console/mouse` (`docs/tty.md`).

## Files

| File | |
|------|-|
| `/dev/fb/ctl` | one line: `width height depth chan pitch mode`, e.g. `1280 800 32 x8r8g8b8 5120 text`; takes `graphics` and `text` |
| `/dev/fb/data` | the pixels, `pitch * height` bytes |

`chan` names the pixel layout from the most significant bits down, as Plan
9 does (`x8r8g8b8`: 8 unused bits, then red, green, blue).

There is one framebuffer, in the mode Limine set up: VBE or GOP on x86_64,
QEMU's `ramfb` through the firmware's GOP on aarch64 and riscv64. There is
no mode setting and no GPU driver. `/dev/fb` exists only when there is a
framebuffer.

A program reaches the pixels by `read` and `write` of `data`
(`cat /dev/fb/data > shot.raw` is a screenshot) or by `mmap`. A
`MAP_SHARED` mapping is the framebuffer itself, shared by every process
that maps it and inherited by a forked child; `MAP_PRIVATE` is a copy.

## Taking the screen

- Writing `graphics` to `ctl` hands the screen to the program: the console
  stops painting text. Console output still goes to serial.
- Writing `text`, or the last close of `ctl`, gives it back. So a program
  holds the screen as long as it keeps `ctl` open, and its exit, a crash
  too, gives it back.

The console then redraws the text screen with what it received meanwhile.

## A desktop on it

TinyX's `Xfbdev` runs on the framebuffer and the raw keyboard
(`get-myos tinyx`, `packages/tinyx/README.md`). Packages on top of it: Xft
(`packages/x11-xft`), dwm with st and dmenu (`get-myos tinyx dwm`, then
`run-myos tinyx,dwm:startx`), `xev` and `xprop` (`x11-apps`) and the surf
browser (`get-myos surf`). Each package's README has the details.

## How it works

A module VFS backend may provide an `mmap` hook (`ModuleVfsOps::mmap`) that
returns the physical page at an offset of a file. For a `MAP_SHARED`
mapping, `do_mmap` (`kernel/src/user/syscall.rs`) maps those pages and
marks the region `MMAP_DEVICE`; munmap, exec and exit unmap them without
freeing them, and fork shares them. Any module can serve device memory this
way; nothing in the kernel knows about framebuffers.

## Limits

- The user mapping is normal write-back memory. That is right for `ramfb`
  (guest RAM). A framebuffer in a PCI BAR on real hardware would want
  write-combining, which there is no type for (a hook can ask for uncached,
  `MYOS_MMAP_UNCACHED`).
- The framebuffer must fit in the process's mmap window (64 GiB,
  `docs/linux-compat.md`).
- Programs written for Linux's `/dev/fb0` ioctls need a shim over these
  files.

## Test

`fb` (`user/c/fb_smoke.c`, tags `console,fb`) is in the mini list on every
arch; the launcher gives every boot a screen.
