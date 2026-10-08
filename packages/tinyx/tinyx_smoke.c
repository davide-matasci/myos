/* tinyx-smoke: boot test of the X server (packages/tinyx), run by test.sh
 * while Xfbdev serves :0.
 *
 * An X client that checks what reaches the hardware: the screen is the size
 * /dev/fb/ctl gives; a window mapped over it with a red background turns the
 * framebuffer's pixels red (read back from /dev/fb/data); and with the
 * window focused, a Shift+A the host types (test.sh asks once this prints
 * "ready") arrives as a KeyPress of keycode 38 (Linux KEY_A, 30, plus 8)
 * that the keymap turns into "A". Prints [ OK ] tinyx. Before the key:
 * MIT-SHM, an image in a System V segment (libgloss shm.c) put to the
 * window, green on the screen, and the window's pixels read back into
 * the segment ([ OK ] tinyx shm).
 */
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/extensions/XShm.h>
#include <sys/ipc.h>
#include <sys/shm.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

static int fail(const char *what) {
    printf("[ FAIL ] tinyx %s\n", what);
    return 1;
}

/* Whether the server listens on display :0's socket yet. Asked with a plain
 * connect: XOpenDisplay would also try TCP (localhost:6000) each time. */
static int server_up(void) {
    struct sockaddr_un a = {.sun_family = AF_UNIX, .sun_path = "/tmp/.X11-unix/X0"};
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    int up = fd >= 0 && connect(fd, (struct sockaddr *)&a, sizeof a) == 0;
    if (fd >= 0) {
        close(fd);
    }
    return up;
}

/* The pixel at (x, y) of the framebuffer, as it reads from data. */
static uint32_t fb_pixel(int fd, unsigned pitch, unsigned x, unsigned y) {
    uint32_t p = 0;
    if (lseek(fd, (off_t)y * pitch + (off_t)x * 4, SEEK_SET) < 0 || read(fd, &p, 4) != 4) {
        return 0xdeadbeef;
    }
    return p;
}

int main(void) {
    char line[128];
    unsigned w, h, depth, pitch;
    int ctl = open("/dev/fb/ctl", O_RDONLY);
    ssize_t n = ctl < 0 ? -1 : read(ctl, line, sizeof line - 1);
    if (n <= 0) {
        return fail("reading /dev/fb/ctl");
    }
    line[n] = '\0';
    close(ctl);
    if (sscanf(line, "%u %u %u %*s %u", &w, &h, &depth, &pitch) != 4 || depth != 32) {
        return fail("/dev/fb/ctl is not a 32-bit screen");
    }

    /* The server may still be starting. */
    for (int i = 0; i < 60 && !server_up(); i++) {
        sleep(1);
    }
    Display *d = XOpenDisplay(":0");
    if (!d) {
        return fail("XOpenDisplay :0");
    }
    int s = DefaultScreen(d);
    if ((unsigned)DisplayWidth(d, s) != w || (unsigned)DisplayHeight(d, s) != h
        || DefaultDepth(d, s) != 24) {
        printf("[ FAIL ] tinyx screen %dx%d depth %d, /dev/fb is %ux%u\n",
               DisplayWidth(d, s), DisplayHeight(d, s), DefaultDepth(d, s), w, h);
        return 1;
    }

    XSetWindowAttributes attr = {.override_redirect = True, .background_pixel = 0xff0000};
    Window win = XCreateWindow(d, RootWindow(d, s), 0, 0, w, h, 0, CopyFromParent, InputOutput,
                               CopyFromParent, CWOverrideRedirect | CWBackPixel, &attr);
    XSelectInput(d, win, KeyPressMask);
    XMapWindow(d, win);
    XSync(d, False);
    XSetInputFocus(d, win, RevertToParent, CurrentTime);
    XSync(d, False);

    /* Not the centre: the pointer starts there, and its cursor is drawn
     * over the window. */
    int fb = open("/dev/fb/data", O_RDONLY);
    uint32_t px = 0;
    for (int i = 0; i < 50 && ((px = fb_pixel(fb, pitch, w / 4, h / 4)) & 0xffffff) != 0xff0000; i++) {
        usleep(100000);
    }
    if ((px & 0xffffff) != 0xff0000) {
        printf("[ FAIL ] tinyx the screen at (%u, %u) is 0x%08x, not red\n", w / 4, h / 4, (unsigned)px);
        return 1;
    }

    /* MIT-SHM: a 64x8 green image in a segment, put at (3w/4, h/4), seen
     * on the screen, and the window's pixels there got back into it. */
    int shm_major, shm_minor;
    Bool shm_pixmaps;
    if (!XShmQueryVersion(d, &shm_major, &shm_minor, &shm_pixmaps)) {
        return fail("no MIT-SHM extension");
    }
    XShmSegmentInfo info;
    XImage *img = XShmCreateImage(d, DefaultVisual(d, s), 24, ZPixmap, NULL, &info, 64, 8);
    if (!img) {
        return fail("XShmCreateImage");
    }
    size_t bytes = (size_t)img->bytes_per_line * img->height;
    info.shmid = shmget(IPC_PRIVATE, bytes, IPC_CREAT | 0600);
    if (info.shmid < 0) {
        return fail("shmget");
    }
    info.shmaddr = img->data = shmat(info.shmid, NULL, 0);
    info.readOnly = False;
    if (info.shmaddr == (void *)-1 || !XShmAttach(d, &info)) {
        return fail("XShmAttach");
    }
    XSync(d, False);
    /* The server has it: the name may go. */
    shmctl(info.shmid, IPC_RMID, NULL);
    for (int y = 0; y < 8; y++) {
        for (int x = 0; x < 64; x++) {
            XPutPixel(img, x, y, 0x00ff00);
        }
    }
    unsigned gx = 3 * w / 4, gy = h / 4;
    GC gc = XCreateGC(d, win, 0, NULL);
    XShmPutImage(d, win, gc, img, 0, 0, gx, gy, 64, 8, False);
    XSync(d, False);
    for (int i = 0; i < 50 && ((px = fb_pixel(fb, pitch, gx + 1, gy + 1)) & 0xffffff) != 0x00ff00; i++) {
        usleep(100000);
    }
    if ((px & 0xffffff) != 0x00ff00) {
        printf("[ FAIL ] tinyx shm: the screen at (%u, %u) is 0x%08x, not green\n", gx + 1, gy + 1, (unsigned)px);
        return 1;
    }
    memset(img->data, 0, bytes);
    if (!XShmGetImage(d, win, img, gx, gy, AllPlanes)) {
        return fail("XShmGetImage");
    }
    if ((XGetPixel(img, 1, 1) & 0xffffff) != 0x00ff00 || (XGetPixel(img, 63, 7) & 0xffffff) != 0x00ff00) {
        printf("[ FAIL ] tinyx shm: XShmGetImage got 0x%08lx, not green\n", XGetPixel(img, 1, 1));
        return 1;
    }
    XShmDetach(d, &info);
    XSync(d, False);
    img->data = NULL;
    XDestroyImage(img);
    shmdt(info.shmaddr);
    printf("[ OK ] tinyx shm\n");

    printf("ready\n");
    fflush(stdout);
    struct pollfd p = {ConnectionNumber(d), POLLIN, 0};
    for (int waited = 0; waited < 60; waited++) {
        while (XPending(d)) {
            XEvent e;
            XNextEvent(d, &e);
            if (e.type != KeyPress) {
                continue;
            }
            char buf[8] = {0};
            KeySym sym;
            int len = XLookupString(&e.xkey, buf, sizeof buf - 1, &sym, NULL);
            if (len == 0) {
                continue; /* Shift */
            }
            if (e.xkey.keycode != 38 || strcmp(buf, "A") != 0) {
                printf("[ FAIL ] tinyx key: keycode %u, \"%s\"\n", e.xkey.keycode, buf);
                return 1;
            }
            XCloseDisplay(d);
            printf("[ OK ] tinyx\n");
            return 0;
        }
        poll(&p, 1, 1000);
    }
    return fail("no KeyPress within 60 s");
}
