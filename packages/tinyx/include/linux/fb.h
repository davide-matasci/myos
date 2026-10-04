/* <linux/fb.h> for TinyX's fbdev server on myos (packages/tinyx).
 *
 * myos has no framebuffer ioctls: /dev/fb is a directory, `ctl` one line of
 * geometry and `data` the pixels (docs/fb.md). This header gives kdrive's
 * fbdev.c the structures and request names it uses, and routes its ioctl()
 * calls to myos_fb_ioctl (kdrive/fbdev/myosfb.c), which answers them from
 * `ctl`. Only the fields fbdev.c touches; the layout is not the kernel's,
 * nothing crosses a kernel boundary. */
#ifndef _MYOS_LINUX_FB_H_
#define _MYOS_LINUX_FB_H_

#include <stdint.h>
#include <sys/ioctl.h>

typedef uint16_t __u16;
typedef uint32_t __u32;

struct fb_bitfield {
    __u32 offset;
    __u32 length;
    __u32 msb_right;
};

struct fb_var_screeninfo {
    __u32 xres, yres, xres_virtual, yres_virtual, xoffset, yoffset;
    __u32 bits_per_pixel, grayscale;
    struct fb_bitfield red, green, blue, transp;
    __u32 nonstd, activate, height, width, accel_flags;
    __u32 pixclock, left_margin, right_margin, upper_margin, lower_margin;
    __u32 hsync_len, vsync_len, sync, vmode, rotate;
};

struct fb_fix_screeninfo {
    char id[16];
    unsigned long smem_start;
    __u32 smem_len, type, type_aux, visual;
    __u16 xpanstep, ypanstep, ywrapstep;
    __u32 line_length;
    unsigned long mmio_start;
    __u32 mmio_len, accel;
};

struct fb_cmap {
    __u32 start, len;
    __u16 *red, *green, *blue, *transp;
};

#define FB_TYPE_PACKED_PIXELS 0
#define FB_VISUAL_MONO01 0
#define FB_VISUAL_MONO10 1
#define FB_VISUAL_TRUECOLOR 2
#define FB_VISUAL_PSEUDOCOLOR 3
#define FB_VISUAL_DIRECTCOLOR 4
#define FB_VISUAL_STATIC_PSEUDOCOLOR 5
#define FB_ACTIVATE_NOW 0
#define FB_CHANGE_CMAP_VBL 32
#define FB_SYNC_HOR_HIGH_ACT 1
#define FB_SYNC_VERT_HIGH_ACT 2
#define FB_BLANK_UNBLANK 0
#define FB_BLANK_POWERDOWN 4

#define FBIOGET_VSCREENINFO 0x4600
#define FBIOPUT_VSCREENINFO 0x4601
#define FBIOGET_FSCREENINFO 0x4602
#define FBIOGETCMAP 0x4604
#define FBIOPUTCMAP 0x4605
#define FBIOBLANK 0x4611

int myos_fb_ioctl(int fd, unsigned long request, ...);
#define ioctl myos_fb_ioctl

#endif
