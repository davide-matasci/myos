/* xft-smoke: the x11-xft package's boot test, an Xft client of Xfbdev.
 *
 * fontconfig resolves "monospace" to DejaVu Sans Mono (the x11-fonts package,
 * fonts.conf), Xft draws white text on a black window over the whole screen,
 * and the framebuffer then has lit pixels in the text's box, with more than
 * two levels of grey (antialiased, through the server's RENDER extension).
 * Prints [ OK ] xft.
 */
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#include <X11/Xlib.h>
#include <X11/Xft/Xft.h>
#include <X11/extensions/Xrender.h>

#define TEXT "Xft text"
#define TX 40
#define TY 80

static int fail(const char *what) {
    printf("[ FAIL ] xft %s\n", what);
    return 1;
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
    for (int i = 0; i < 60 && !server_up(); i++) {
        sleep(1);
    }
    Display *dpy = XOpenDisplay(":0");
    if (!dpy) {
        return fail("XOpenDisplay");
    }
    int scr = DefaultScreen(dpy), ev, err;
    int render = XRenderQueryExtension(dpy, &ev, &err);

    XftFont *font = XftFontOpenName(dpy, scr, "monospace:size=24");
    FcChar8 *family = NULL;
    if (!font || FcPatternGetString(font->pattern, FC_FAMILY, 0, &family) != FcResultMatch) {
        return fail("no monospace font");
    }
    if (strcmp((const char *)family, "DejaVu Sans Mono") != 0) {
        printf("[ FAIL ] xft monospace is %s, not DejaVu Sans Mono\n", family);
        return 1;
    }

    XSetWindowAttributes wa = {.override_redirect = True, .background_pixel = BlackPixel(dpy, scr)};
    Window win = XCreateWindow(dpy, RootWindow(dpy, scr), 0, 0, w, h, 0, CopyFromParent, InputOutput,
                               CopyFromParent, CWOverrideRedirect | CWBackPixel, &wa);
    XMapWindow(dpy, win);
    XSync(dpy, False);
    XftDraw *draw = XftDrawCreate(dpy, win, DefaultVisual(dpy, scr), DefaultColormap(dpy, scr));
    XftColor white;
    if (!draw || !XftColorAllocName(dpy, DefaultVisual(dpy, scr), DefaultColormap(dpy, scr), "#ffffff", &white)) {
        return fail("XftDraw / color");
    }
    XGlyphInfo ext;
    XftTextExtentsUtf8(dpy, font, (const FcChar8 *)TEXT, strlen(TEXT), &ext);

    /* The text's box, read back from the framebuffer until it is drawn. */
    int fd = open("/dev/fb/data", O_RDONLY);
    int x0 = TX - ext.x, y0 = TY - ext.y, bw = ext.width, bh = ext.height;
    if (fd < 0 || bw <= 0 || bh <= 0 || x0 < 0 || y0 < 0 || x0 + bw > (int)w || y0 + bh > (int)h || bw > 2048) {
        return fail("text box");
    }
    int lit = 0, levels = 0;
    for (int tries = 0; tries < 30 && (lit < 50 || levels < 3); tries++) {
        XftDrawStringUtf8(draw, &white, font, TX, TY, (const FcChar8 *)TEXT, strlen(TEXT));
        XSync(dpy, False);
        unsigned char seen[256] = {0};
        uint32_t row[2048];
        lit = levels = 0;
        for (int y = y0; y < y0 + bh; y++) {
            if (lseek(fd, (off_t)y * pitch + x0 * 4, SEEK_SET) < 0 || read(fd, row, bw * 4) != bw * 4) {
                return fail("reading /dev/fb/data");
            }
            for (int x = 0; x < bw; x++) {
                unsigned g = (row[x] >> 8) & 0xff;
                lit += g != 0;
                if (!seen[g]) {
                    seen[g] = 1;
                    levels++;
                }
            }
        }
        if (lit < 50 || levels < 3) {
            sleep(1);
        }
    }
    if (lit < 50 || levels < 3) {
        printf("[ FAIL ] xft text not on the screen: %d lit pixels, %d levels (RENDER %s)\n", lit, levels,
               render ? "yes" : "no");
        return 1;
    }
    printf("[ OK ] xft %s, %d lit pixels, %d levels, RENDER %s\n", family, lit, levels, render ? "yes" : "no");
    return 0;
}
