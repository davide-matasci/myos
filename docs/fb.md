# Framebuffer: `/dev/fb`

`/dev/fb` is the screen, for programs that draw on it themselves. It is the
first step towards a GUI. Like `/net`, it is a directory of files rather
than a device with ioctls.

The console module serves it (`modules/console/src/fbdev.rs`), since it
already owns the screen and paints the text console on it. A program on the
screen takes its input from two other files, both in `docs/tty.md`:

- `/dev/console/kbd`: the keyboard's presses and releases;
- `/dev/console/mouse`: the pointer.

## Files

| File | |
|------|-|
| `/dev/fb/ctl` | one line: `width height depth chan pitch mode`, e.g. `1280 800 32 x8r8g8b8 5120 text`; takes `graphics` and `text` |
| `/dev/fb/data` | the pixels, `pitch * height` bytes |

### The framebuffer

There is one framebuffer, in one mode: the one Limine set up. There is no
mode setting and no GPU driver. `/dev/fb` exists only when there is a
framebuffer.

| Arch | Framebuffer |
|------|-------------|
| x86_64 | VBE (BIOS) or GOP (UEFI) |
| aarch64, riscv64 | QEMU's `ramfb`, through the firmware's GOP |

The fields of the `ctl` line:

- `chan` names the pixel layout from the most significant bits down, as
  Plan 9 does. `x8r8g8b8` is 32 bits: 8 unused, then red, green and blue.
- `depth` is the bits per pixel.
- `pitch` is the bytes per row.

### Pixels

A program reaches the pixels in two ways:

- `read` and `write` of `data` at an offset. `cat /dev/fb/data > shot.raw`
  is a screenshot.
- `mmap` of `data`:
  - `MAP_SHARED` maps the framebuffer itself (any page-aligned window of
    it). Every process mapping it shares the same pixels, and a forked
    child inherits the mapping as shared.
  - `MAP_PRIVATE` gives a copy of the pixels, as for any file.

### Taking the screen

The console paints its text on the same pixels. A program takes the screen
from it through `ctl`:

- Writing `graphics` to `ctl` hands the screen to the program. The console
  module stops painting text and blinking the cursor. Console output still
  goes to serial.
- Writing `text` gives the screen back.
- The last close of `ctl` gives it back too. A program holds the screen as
  long as it keeps `ctl` open, and its exit (a crash too) gives it back.

On the way back, the console draws the text screen shown again, with what
it received meanwhile (`docs/tty.md`). `echo graphics > /dev/fb/ctl` from a
shell takes the screen and gives it back at once.

## A desktop on it

An X server runs on the framebuffer and the raw keyboard: TinyX's
`Xfbdev`, with MIT-SHM (`get-myos tinyx`, `packages/tinyx/README.md`).
On top of it, as packages:

- antialiased TrueType text through Xft (`packages/x11-xft/README.md`);
- the dwm window manager, the st terminal and the dmenu menu dwm starts
  (`packages/dwm/README.md`, `packages/st/README.md`,
  `packages/dmenu/README.md`). `get-myos tinyx dwm` brings st, dmenu, the
  Xft stack and the fonts with it; `run-myos tinyx,dwm:startx` starts the
  session;
- `xev`, which prints the events a window gets, and `xprop`, which prints
  its properties (`get-myos x11-apps`, then `run-myos x11-apps`,
  `packages/x11-apps/README.md`);
- the surf web browser on WebKitGTK (`get-myos surf`, then `run-myos surf
  URL` in dwm's terminal, `packages/surf/README.md`).

## How it works

- **The mount**: the console module mounts its files at `dev/fb`
  (`vfs_mount`). The VFS sends a path to the longest matching mount prefix,
  so `/dev/fb/...` reaches the module while the rest of `/dev` stays devfs.
- **Device mappings**: a module VFS backend may provide an `mmap` hook
  (`ModuleVfsOps::mmap`, ABI 16). It returns the physical page at an offset
  of a file; the console's returns the framebuffer's pages.
  - For a `MAP_SHARED` mapping of such a file, `do_mmap`
    (`kernel/src/user/syscall.rs`) maps those pages instead of allocating,
    and marks the region `MMAP_DEVICE` (`kernel/src/task/mod.rs`).
  - The pages stay the device's. munmap, exec and exit unmap them without
    freeing them (`release_mmap_range`, `free_mmap_regions` in
    `kernel/src/user/aspace.rs`).
  - Fork maps the same pages into the child (`share_mmap_pages`).
  - Any module can serve device memory this way. Nothing in the kernel
    knows about framebuffers.
- **Ownership**: `ctl`'s `release` hook runs when the last fd of the path
  is closed, across forks, and returns the screen to text. The console
  module checks its graphics flag before every paint and cursor blink.

## Limits

- The user mapping uses the same memory type as other user pages (normal,
  write-back). That is right for `ramfb`, which is guest RAM, and fine
  under QEMU. Real hardware with the framebuffer in a PCI BAR would want a
  write-combining mapping. A hook can ask for an uncached one
  (`MYOS_MMAP_UNCACHED`, as `/dev/pci`'s registers do), but there is no
  write-combining type.
- The framebuffer must fit in the process's mmap window (64 GiB,
  `docs/linux-compat.md`).
- Programs written for Linux's `/dev/fb0` (fbdev ioctls) need a small shim
  over these files, as the socket library is one over `/net`.

## Test

The `fb` test (`user/c/fb_smoke.c`, `t -t console,fb fb` in
`user/c/test.sh`) is in the mini list on every arch. The launcher gives
every boot a screen, the headless CI ones too. It checks:

- the `ctl` line;
- a `MAP_SHARED` mapping of `data` that reads back through `read` and sees
  what `write` put there;
- a forked child drawing into the same pixels;
- a `MAP_PRIVATE` mapping that is a copy;
- the screen going back to text by a `text` write, by closing `ctl` and by
  the exit of a child holding it.
