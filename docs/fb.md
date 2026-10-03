# Framebuffer: `/dev/fb0`

The boot framebuffer is available to userspace as `/dev/fb0`, with the
Linux fbdev interface (same ioctl numbers and struct layouts), so a program
written for Linux's `/dev/fb0` builds against newlib unchanged
(`<linux/fb.h>`, `<linux/kd.h>` from libgloss). It is the first step
towards a GUI: a graphics program, or a window system's server, draws here.

## What there is

- **One framebuffer, one mode**: the one Limine set up: VBE (BIOS) or GOP
  (UEFI) on x86_64, QEMU's `ramfb` through the firmware's GOP on aarch64
  and riscv64. No mode setting and no GPU driver. The node exists only when
  Limine gave us a framebuffer (with a page-aligned base).
- **Geometry**: `FBIOGET_VSCREENINFO` (resolution, bits per pixel, the
  red/green/blue bit fields) and `FBIOGET_FSCREENINFO` (`line_length`,
  `smem_len`, `smem_start`). `FBIOPUT_VSCREENINFO` succeeds only for the
  current resolution and depth; `FBIOPAN_DISPLAY` and `FBIOBLANK` are
  accepted and do nothing.
- **Pixels**: `mmap(NULL, smem_len, PROT_READ|PROT_WRITE, MAP_SHARED, fd, 0)`
  maps the framebuffer's own memory (any page-aligned window of it). Every
  process mapping it shares the same pixels, and a forked child inherits the
  mapping as shared rather than copied. `read`/`write` at an offset work too:
  `cat /dev/fb0 > shot.raw` is a screenshot.
- **The console**: the console module paints text on the same pixels. A
  program takes the screen with `ioctl(fd, KDSETMODE, KD_GRAPHICS)` (on
  `/dev/fb0` or on its console tty) and gives it back with `KD_TEXT`;
  `KDGETMODE` reads the mode. In graphics mode console output still goes to
  serial, but nothing is painted and the cursor does not blink. When the
  process that set graphics mode exits, the console returns to text mode by
  itself, so a crashed program does not leave the screen dark. Going back
  to text clears the screen (the text console cannot repaint what was drawn
  over it).

## How it works

`kernel/src/fb.rs` holds the device: the geometry ioctls, `read`/`write`,
and `frame_at`, which `do_mmap` (`kernel/src/user/syscall.rs`) uses to map
the framebuffer's physical pages into the caller instead of allocating
fresh ones. Those pages are not the frame allocator's: `free_mapped_page`
(munmap, exec, exit) unmaps them without freeing them, and the fork copy
(`copy_mmap_pages`) maps the same frames into the child. devfs
(`kernel/src/fs/devfs.rs`) lists the node; `task::fd_ioctl` routes its
ioctls, which copy structs to and from userspace, to `fb.rs`. The graphics
mode lives in `kernel/src/console.rs` (`set_graphics`), and `task::die`
reports every exiting process to it.

## Limits

- The user mapping uses the same memory type as other user pages (normal,
  write-back). That is right for `ramfb`, which is guest RAM, and fine
  under QEMU; real hardware with the framebuffer in a PCI BAR would want a
  write-combining or uncached mapping.
- The framebuffer must fit in the process's mmap window (128 MiB on
  x86_64, 64 MiB on aarch64 and riscv64).
- No mouse yet, and keyboard input still reaches programs only as tty bytes
  (`kernel/src/input.rs`); a raw input device is the next step for a GUI.
- The Linux layer passes the ioctls through, but its `mmap` still refuses
  every `MAP_SHARED` file mapping (`modules/linux/src/sys.rs`), `/dev/fb0`
  included.

## Test

`user/c/fb_smoke.c` (`t fb` in `user/c/test.sh`, in the mini list on every
arch): the geometry, a `MAP_SHARED` mapping that reads back through `read`
and sees what `write` put there, a forked child drawing into the same
pixels, `KD_GRAPHICS`/`KD_TEXT`, and a child that exits in graphics mode
handing the screen back.
