/* surf_check: packages/surf's boot test helper, on display :0 (Xlib).
 *   surf_check server        wait (up to 60 s) for the X server's socket,
 *                            so that surf (which does not retry) finds it
 *   surf_check page XID      wait (up to 600 s) for surf's window XID to
 *                            show the launcher's test page: its title
 *                            surf's "... | myos test page" with the load
 *                            done (no "[N%]" before it), and the page's
 *                            blue box at (100, 50)
 *   surf_check uri XID URI   wait (up to 300 s) for the window's
 *                            _SURF_URI (the page surf shows) to be URI
 *
 * Prints "[ OK ] surf <mode>", or what it last saw.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#include <X11/Xlib.h>
#include <X11/Xutil.h>

#define TITLE "| myos test page"

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

/* The window's text property NAME, in buf (empty when it has none). */
static void property(Display *dpy, Window w, const char *name, char *buf, size_t size) {
    Atom type;
    int format;
    unsigned long n, left;
    unsigned char *data = NULL;
    buf[0] = '\0';
    if (XGetWindowProperty(dpy, w, XInternAtom(dpy, name, False), 0, 1024, False, AnyPropertyType,
                           &type, &format, &n, &left, &data) == Success && data) {
        if (format == 8) {
            snprintf(buf, size, "%.*s", (int)n, (char *)data);
        }
        XFree(data);
    }
}

/* Whether the window shows pure blue at (x, y). */
static int blue_at(Display *dpy, Window w, int x, int y) {
    XWindowAttributes wa;
    XImage *img;
    unsigned long p;
    int blue;
    if (!XGetWindowAttributes(dpy, w, &wa) || x >= wa.width || y >= wa.height) {
        return 0;
    }
    img = XGetImage(dpy, w, x, y, 1, 1, AllPlanes, ZPixmap);
    if (!img) {
        return 0;
    }
    p = XGetPixel(img, 0, 0);
    blue = !(p & wa.visual->red_mask) && !(p & wa.visual->green_mask)
        && (p & wa.visual->blue_mask) == wa.visual->blue_mask;
    XDestroyImage(img);
    return blue;
}

static int ignore(Display *dpy, XErrorEvent *e) {
    (void)dpy;
    (void)e;
    return 0;
}

int main(int argc, char **argv) {
    char seen[1024] = "";
    Display *dpy;
    Window w;
    int i;
    if (argc == 2 && !strcmp(argv[1], "server")) {
        for (i = 0; i < 60 && !server_up(); i++) {
            sleep(1);
        }
        printf(server_up() ? "[ OK ] surf server\n" : "[ FAIL ] surf server: no X server on :0\n");
        return !server_up();
    }
    if (argc < 3 || (strcmp(argv[1], "page") && (strcmp(argv[1], "uri") || argc != 4))) {
        fprintf(stderr, "usage: surf_check server | page XID | uri XID URI\n");
        return 2;
    }
    if (!(dpy = XOpenDisplay(":0"))) {
        printf("[ FAIL ] surf %s: cannot open display :0\n", argv[1]);
        return 1;
    }
    /* The window can go while surf loads: a BadWindow is a reply, not an end. */
    XSetErrorHandler(ignore);
    w = strtoul(argv[2], NULL, 0);
    for (i = 0; i < (argv[1][0] == 'p' ? 600 : 300); i++) {
        if (argv[1][0] == 'p') {
            size_t n, t = strlen(TITLE);
            property(dpy, w, "_NET_WM_NAME", seen, sizeof seen);
            n = strlen(seen);
            if (seen[0] != '[' && n >= t && !strcmp(seen + n - t, TITLE) && blue_at(dpy, w, 100, 50)) {
                printf("[ OK ] surf page: \"%s\"\n", seen);
                return 0;
            }
        } else {
            property(dpy, w, "_SURF_URI", seen, sizeof seen);
            if (!strcmp(seen, argv[3])) {
                printf("[ OK ] surf uri: %s\n", seen);
                return 0;
            }
        }
        sleep(1);
    }
    printf("[ FAIL ] surf %s: the window shows \"%s\"\n", argv[1], seen);
    return 1;
}
