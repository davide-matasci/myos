/* image-smoke: the image-libs package's boot test, against mlibc.
 *
 * pixman fills and composites a 64x64 image (an opaque red square, half
 * blue over a quarter of it); libpng and libwebp (lossless) round-trip it
 * exactly through memory, libjpeg-turbo a gradient within its loss. Then,
 * on Xfbdev (display :0), through x11-libs' mlibc build: the image goes
 * up with MIT-SHM (XShmPutImage), RENDER fills a green square, Xcursor
 * makes a cursor from an ARGB image, and XGetImage reads the pixels back;
 * the other extension libraries (XFIXES, RandR, XInput, Xinerama,
 * Composite, DAMAGE) are asked for the server's version where it has
 * them. Prints [ OK ] image.
 */
#include <setjmp.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ipc.h>
#include <sys/shm.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <jpeglib.h>
#include <pixman.h>
#include <png.h>
#include <webp/decode.h>
#include <webp/encode.h>

#include <X11/Xlib.h>
#include <X11/Xcursor/Xcursor.h>
#include <X11/Xutil.h>
#include <X11/extensions/XInput2.h>
#include <X11/extensions/XShm.h>
#include <X11/extensions/Xcomposite.h>
#include <X11/extensions/Xdamage.h>
#include <X11/extensions/Xfixes.h>
#include <X11/extensions/Xinerama.h>
#include <X11/extensions/Xrandr.h>
#include <X11/extensions/Xrender.h>

#define W 64
#define H 64

static int fail(const char *what) {
    printf("[ FAIL ] image %s\n", what);
    return 1;
}

static int near(uint32_t got, uint32_t want, int tolerance) {
    for (int shift = 0; shift < 32; shift += 8) {
        int d = (int)((got >> shift) & 0xff) - (int)((want >> shift) & 0xff);
        if (d > tolerance || d < -tolerance) {
            return 0;
        }
    }
    return 1;
}

/* pixman: red everywhere, then a half-transparent blue over the top-left
 * quarter. */
static int draw(uint32_t *bits) {
    pixman_image_t *img = pixman_image_create_bits(PIXMAN_a8r8g8b8, W, H, bits, W * 4);
    pixman_color_t red = {0xffff, 0, 0, 0xffff}, blue = {0, 0, 0x8000, 0x8000};
    pixman_box32_t all = {0, 0, W, H};
    pixman_image_t *src = pixman_image_create_solid_fill(&blue);
    if (!img || !src || !pixman_image_fill_boxes(PIXMAN_OP_SRC, img, &red, 1, &all)) {
        return fail("pixman images");
    }
    pixman_image_composite32(PIXMAN_OP_OVER, src, NULL, img, 0, 0, 0, 0, 0, 0, W / 2, H / 2);
    pixman_image_unref(src);
    pixman_image_unref(img);
    if (bits[(H - 1) * W + W - 1] != 0xffff0000 || !near(bits[0], 0xff7f0080, 1)) {
        printf("[ FAIL ] image pixman pixels %08x %08x\n", bits[0], bits[(H - 1) * W + W - 1]);
        return 1;
    }
    return 0;
}

/* libpng's simplified API, through memory. */
static int png_round_trip(const uint32_t *bits) {
    png_image img = {.version = PNG_IMAGE_VERSION, .width = W, .height = H, .format = PNG_FORMAT_BGRA};
    png_alloc_size_t size = 0;
    if (!png_image_write_to_memory(&img, NULL, &size, 0, bits, W * 4, NULL)) {
        return fail("png size");
    }
    void *png = malloc(size);
    uint32_t back[W * H];
    if (!png || !png_image_write_to_memory(&img, png, &size, 0, bits, W * 4, NULL)) {
        return fail("png write");
    }
    png_image in = {.version = PNG_IMAGE_VERSION};
    if (!png_image_begin_read_from_memory(&in, png, size) || in.width != W || in.height != H) {
        return fail("png header");
    }
    in.format = PNG_FORMAT_BGRA;
    if (!png_image_finish_read(&in, NULL, back, W * 4, NULL) || memcmp(back, bits, sizeof back) != 0) {
        return fail("png pixels");
    }
    free(png);
    return 0;
}

/* libwebp, lossless, through memory. */
static int webp_round_trip(const uint32_t *bits) {
    uint8_t *webp = NULL;
    size_t size = WebPEncodeLosslessBGRA((const uint8_t *)bits, W, H, W * 4, &webp);
    int w = 0, h = 0;
    uint8_t *back = size ? WebPDecodeBGRA(webp, size, &w, &h) : NULL;
    int ok = back && w == W && h == H && memcmp(back, bits, W * H * 4) == 0;
    WebPFree(back);
    WebPFree(webp);
    return ok ? 0 : fail("webp round trip");
}

struct jpeg_error {
    struct jpeg_error_mgr mgr;
    jmp_buf jump;
};

static void jpeg_fail(j_common_ptr cinfo) {
    longjmp(((struct jpeg_error *)cinfo->err)->jump, 1);
}

/* libjpeg-turbo: a gradient through memory, back within the loss. */
static int jpeg_round_trip(void) {
    static uint8_t rgb[W * H * 3], back[W * H * 3];
    for (int y = 0; y < H; y++) {
        for (int x = 0; x < W; x++) {
            uint8_t *p = &rgb[(y * W + x) * 3];
            p[0] = x * 4, p[1] = y * 4, p[2] = 128;
        }
    }
    struct jpeg_compress_struct c;
    struct jpeg_decompress_struct d;
    struct jpeg_error err;
    unsigned char *jpg = NULL;
    unsigned long size = 0;
    c.err = jpeg_std_error(&err.mgr);
    d.err = c.err;
    err.mgr.error_exit = jpeg_fail;
    if (setjmp(err.jump)) {
        return fail("jpeg error");
    }
    jpeg_create_compress(&c);
    jpeg_mem_dest(&c, &jpg, &size);
    c.image_width = W, c.image_height = H, c.input_components = 3, c.in_color_space = JCS_RGB;
    jpeg_set_defaults(&c);
    jpeg_set_quality(&c, 95, TRUE);
    jpeg_start_compress(&c, TRUE);
    while (c.next_scanline < H) {
        JSAMPROW row = &rgb[c.next_scanline * W * 3];
        jpeg_write_scanlines(&c, &row, 1);
    }
    jpeg_finish_compress(&c);
    jpeg_destroy_compress(&c);

    jpeg_create_decompress(&d);
    jpeg_mem_src(&d, jpg, size);
    jpeg_read_header(&d, TRUE);
    jpeg_start_decompress(&d);
    if (d.output_width != W || d.output_height != H || d.output_components != 3) {
        return fail("jpeg header");
    }
    while (d.output_scanline < H) {
        JSAMPROW row = &back[d.output_scanline * W * 3];
        jpeg_read_scanlines(&d, &row, 1);
    }
    jpeg_finish_decompress(&d);
    jpeg_destroy_decompress(&d);
    free(jpg);
    for (int i = 0; i < W * H * 3; i++) {
        if (abs(rgb[i] - back[i]) > 16) {
            return fail("jpeg pixels");
        }
    }
    return 0;
}

/* Whether the server listens on display :0's socket yet. */
static int server_up(void) {
    struct sockaddr_un a = {.sun_family = AF_UNIX, .sun_path = "/tmp/.X11-unix/X0"};
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    int up = fd >= 0 && connect(fd, (struct sockaddr *)&a, sizeof a) == 0;
    if (fd >= 0) {
        close(fd);
    }
    return up;
}

/* The pixel at (x, y) of the window, 0xRRGGBB. */
static uint32_t pixel_at(Display *dpy, Window win, int x, int y) {
    XImage *img = XGetImage(dpy, win, x, y, 1, 1, AllPlanes, ZPixmap);
    uint32_t p = img ? (uint32_t)XGetPixel(img, 0, 0) & 0xffffff : 0xdeadbeef;
    if (img) {
        XDestroyImage(img);
    }
    return p;
}

static int x11(const uint32_t *bits, char *exts, size_t cap) {
    for (int i = 0; i < 60 && !server_up(); i++) {
        sleep(1);
    }
    Display *dpy = XOpenDisplay(":0");
    if (!dpy) {
        return fail("XOpenDisplay");
    }
    int scr = DefaultScreen(dpy), ev, err, major, minor;
    Bool pixmaps;
    if (!XShmQueryVersion(dpy, &major, &minor, &pixmaps) || !XRenderQueryExtension(dpy, &ev, &err)) {
        return fail("no MIT-SHM or RENDER on the server");
    }
    Visual *vis = DefaultVisual(dpy, scr);
    XSetWindowAttributes wa = {.override_redirect = True, .background_pixel = BlackPixel(dpy, scr)};
    Window win = XCreateWindow(dpy, RootWindow(dpy, scr), 0, 0, 320, 160, 0, CopyFromParent, InputOutput,
                               CopyFromParent, CWOverrideRedirect | CWBackPixel, &wa);
    XMapWindow(dpy, win);
    XSync(dpy, False);

    /* The pixman image, over MIT-SHM. */
    XShmSegmentInfo shm = {0};
    XImage *img = XShmCreateImage(dpy, vis, DefaultDepth(dpy, scr), ZPixmap, NULL, &shm, W, H);
    if (!img || img->bits_per_pixel != 32) {
        return fail("XShmCreateImage");
    }
    shm.shmid = shmget(IPC_PRIVATE, img->bytes_per_line * img->height, IPC_CREAT | 0600);
    shm.shmaddr = img->data = shmat(shm.shmid, NULL, 0);
    if (shm.shmid < 0 || shm.shmaddr == (char *)-1 || !XShmAttach(dpy, &shm)) {
        return fail("shmget / XShmAttach");
    }
    for (int y = 0; y < H; y++) {
        memcpy(img->data + y * img->bytes_per_line, bits + y * W, W * 4);
    }
    XShmPutImage(dpy, win, DefaultGC(dpy, scr), img, 0, 0, 16, 16, W, H, False);

    /* RENDER: a green square beside it. */
    XRenderPictFormat *fmt = XRenderFindVisualFormat(dpy, vis);
    Picture pic = XRenderCreatePicture(dpy, win, fmt, 0, NULL);
    XRenderColor green = {0, 0xffff, 0, 0xffff};
    XRenderFillRectangle(dpy, PictOpSrc, pic, &green, 160, 16, 32, 32);

    /* Xcursor: an ARGB cursor (over RENDER's CreateCursor). */
    XcursorImage *ci = XcursorImageCreate(16, 16);
    for (int i = 0; ci && i < 16 * 16; i++) {
        ci->pixels[i] = 0xff0000ff;
    }
    Cursor cursor = ci ? XcursorImageLoadCursor(dpy, ci) : None;
    if (cursor == None) {
        return fail("Xcursor");
    }
    XDefineCursor(dpy, win, cursor);
    XSync(dpy, False);

    uint32_t red = pixel_at(dpy, win, 16 + W - 1, 16 + H - 1);
    uint32_t mixed = pixel_at(dpy, win, 16, 16);
    uint32_t got = pixel_at(dpy, win, 170, 30);
    if (red != 0xff0000 || !near(mixed, 0x7f0080, 1) || got != 0x00ff00) {
        printf("[ FAIL ] image on the screen: %06x %06x %06x\n", red, mixed, got);
        return 1;
    }

    /* The other extensions' libraries, where the server has them. */
    struct {
        const char *name;
        Bool (*query)(Display *, int *, int *);
    } q[] = {
        {"XFIXES", XFixesQueryExtension}, {"RANDR", XRRQueryExtension},
        {"XINERAMA", XineramaQueryExtension}, {"Composite", XCompositeQueryExtension},
        {"DAMAGE", XDamageQueryExtension},
    };
    int opcode;
    exts[0] = '\0';
    for (size_t i = 0; i < sizeof q / sizeof q[0]; i++) {
        if (q[i].query(dpy, &ev, &err)) {
            snprintf(exts + strlen(exts), cap - strlen(exts), " %s", q[i].name);
        }
    }
    if (XQueryExtension(dpy, "XInputExtension", &opcode, &ev, &err)) {
        major = 2, minor = 0;
        if (XIQueryVersion(dpy, &major, &minor) == Success) {
            snprintf(exts + strlen(exts), cap - strlen(exts), " XInput%d.%d", major, minor);
        }
    }

    XShmDetach(dpy, &shm);
    shmdt(shm.shmaddr);
    shmctl(shm.shmid, IPC_RMID, NULL);
    XCloseDisplay(dpy);
    return 0;
}

int main(void) {
    static uint32_t bits[W * H];
    char exts[128];
    if (draw(bits) || png_round_trip(bits) || webp_round_trip(bits) || jpeg_round_trip()
        || x11(bits, exts, sizeof exts)) {
        return 1;
    }
    printf("[ OK ] image (server:%s)\n", exts);
    return 0;
}
