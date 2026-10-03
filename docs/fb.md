# Framebuffer: `/dev/fb`

The screen, for programs that draw on it themselves: the first step towards
a GUI. Like `/net`, it is a directory of files rather than a device with
ioctls. The console module serves it (`modules/console/src/fbdev.rs`), since
it already owns the screen and paints the text console on it.

## Files

| File | |
|------|-|
| `/dev/fb/ctl` | one line: `width height depth chan pitch mode`, e.g. `1280 800 32 x8r8g8b8 5120 text`; takes `graphics` and `text` |
| `/dev/fb/data` | the pixels, `pitch * height` bytes |

- **One framebuffer, one mode**: the one Limine set up: VBE (BIOS) or GOP
  (UEFI) on x86_64, QEMU's `ramfb` through the firmware's GOP on aarch64
  and riscv64. No mode setting and no GPU driver. `/dev/fb` exists only
  when there is a framebuffer.
- **`chan`** names the pixel layout from the most significant bits down,
  as Plan 9 does: `x8r8g8b8` is 32 bits, 8 unused, then red, green and
  blue. `depth` is the bits per pixel, `pitch` the bytes per row.
- **Pixels**: `read`/`write` of `data` at an offset (`cat /dev/fb/data >
  shot.raw` is a screenshot), or `mmap`. `MAP_SHARED` maps the framebuffer
  itself (any page-aligned window of it): every process mapping it shares
  the same pixels, and a forked child inherits the mapping as shared.
  `MAP_PRIVATE` gives a copy of the pixels, as for any file.
- **Taking the screen**: the console paints its text on the same pixels.
  Writing `graphics` to `ctl` hands the screen to the program: the console
  module stops painting text and blinking the cursor (console output still
  goes to serial). Writing `text` gives it back, and so does the last close
  of `ctl`: a program holds the screen as long as it keeps `ctl` open, and
  its exit (a crash too) gives it back. The screen is cleared on the way
  back, since the text console cannot repaint what was drawn over it.
  (`echo graphics > /dev/fb/ctl` from a shell takes it and gives it back
  at once.)

## How it works

- **The mount**: the console module mounts its files at `dev/fb`
  (`vfs_mount`); the VFS sends paths to the longest matching mount prefix,
  so `/dev/fb/...` reaches the module while the rest of `/dev` stays devfs.
- **Device mappings**: a module VFS backend may provide an `mmap` hook
  (`ModuleVfsOps::mmap`, ABI 16): the physical page at an offset of a file.
  The console's gives the framebuffer's pages. For a `MAP_SHARED` mapping
  of such a file, `do_mmap` (`kernel/src/user/syscall.rs`) maps those
  pages instead of allocating, and marks the region `MMAP_DEVICE`
  (`kernel/src/task/mod.rs`). The pages stay the device's: munmap, exec
  and exit unmap them without freeing (`release_mmap_range`,
  `free_mmap_regions` in `kernel/src/user/aspace.rs`), and fork maps the
  same pages into the child (`copy_mmap_pages`). Any module can serve
  device memory this way; nothing in the kernel knows about framebuffers.
- **Ownership**: `ctl`'s `release` hook (the last fd of the path closed,
  across forks) returns the screen to text; the console module checks its
  graphics flag before every paint and cursor blink.

## Limits

- The user mapping uses the same memory type as other user pages (normal,
  write-back). That is right for `ramfb`, which is guest RAM, and fine
  under QEMU; real hardware with the framebuffer in a PCI BAR would want a
  write-combining or uncached mapping.
- The framebuffer must fit in the process's mmap window (128 MiB on
  x86_64, 64 MiB on aarch64 and riscv64).
- No mouse yet, and keyboard input still reaches programs only as tty bytes
  (`kernel/src/input.rs`); a raw input device is the next step for a GUI.
- The Linux layer's `mmap` still refuses every `MAP_SHARED` file mapping
  (`modules/linux/src/sys.rs`), `/dev/fb/data` included.
- Programs written for Linux's `/dev/fb0` (fbdev ioctls) need a small shim
  over these files, as the socket library is one over `/net`.

## Test

`user/c/fb_smoke.c` (`t fb` in `user/c/test.sh`, in the mini list on every
arch: the launcher gives every boot, the headless CI ones too, a screen):
the `ctl` line, a `MAP_SHARED` mapping of `data` that reads back through
`read` and sees what `write` put there, a forked child drawing into the
same pixels, a `MAP_PRIVATE` mapping that is a copy, and the screen going
back to text by a `text` write, by closing `ctl` and by the exit of a child
holding it.
