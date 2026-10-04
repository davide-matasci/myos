/* x11-smoke: boot test of the X client libraries (packages/x11-libs).
 *
 * There is no X server to talk to yet, so this program is both ends: the
 * parent listens on /tmp/.X11-unix/X5 (display :5) and plays a server that
 * knows just enough of the protocol (the connection setup with one 640x480
 * TrueColor screen, replies to QueryExtension, GetProperty, InternAtom and
 * GetInputFocus); the child is a libX11 client: XOpenDisplay, the screen it
 * was told about, a window created, named and mapped, an atom interned, a
 * round trip, XCloseDisplay. The server checks what arrived (the window's
 * size, its WM_NAME, the map). That is libxcb's and libX11's whole path:
 * the setup, request encoding, writev, poll and the replies. Prints
 * [ OK ] x11.
 */
#include <X11/Xatom.h>
#include <X11/Xlib.h>
#include <errno.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

#define SOCKET "/tmp/.X11-unix/X5"
#define WIDTH 640
#define HEIGHT 480
#define VENDOR "myos x11-smoke"
#define ROOT_WINDOW 0x40
#define ROOT_COLORMAP 0x41
#define ROOT_VISUAL 0x42
#define FIRST_ATOM 100

/* ---- the stand-in server ---- */

static int read_full(int fd, void *buf, size_t n) {
    size_t got = 0;
    while (got < n) {
        ssize_t r = read(fd, (char *)buf + got, n - got);
        if (r > 0) {
            got += (size_t)r;
        } else if (r == 0) {
            return got ? -1 : 0;
        } else if (errno == EAGAIN || errno == EINTR) {
            struct pollfd p = {fd, POLLIN, 0};
            poll(&p, 1, 1000);
        } else {
            return -1;
        }
    }
    return 1;
}

static int write_full(int fd, const void *buf, size_t n) {
    size_t done = 0;
    while (done < n) {
        ssize_t w = write(fd, (const char *)buf + done, n - done);
        if (w > 0) {
            done += (size_t)w;
        } else if (w < 0 && (errno == EAGAIN || errno == EINTR)) {
            struct pollfd p = {fd, POLLOUT, 0};
            poll(&p, 1, 1000);
        } else {
            return -1;
        }
    }
    return 0;
}

static void put16(uint8_t *p, uint16_t v) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
}

static void put32(uint8_t *p, uint32_t v) {
    put16(p, (uint16_t)v);
    put16(p + 2, (uint16_t)(v >> 16));
}

static uint16_t get16(const uint8_t *p) {
    return (uint16_t)(p[0] | p[1] << 8);
}

static uint32_t get32(const uint8_t *p) {
    return get16(p) | (uint32_t)get16(p + 2) << 16;
}

static size_t pad4(size_t n) {
    return (n + 3) & ~(size_t)3;
}

/* The connection setup reply: one screen, one depth, one TrueColor visual. */
static size_t setup_reply(uint8_t *r) {
    memset(r, 0, 256);
    uint8_t *p = r + 8;
    put32(p + 4, 0x00200000);           /* resource-id-base */
    put32(p + 8, 0x001fffff);           /* resource-id-mask */
    put16(p + 16, sizeof VENDOR - 1);
    put16(p + 18, 0xffff);              /* maximum-request-length */
    p[20] = 1;                          /* screens */
    p[21] = 2;                          /* pixmap formats */
    p[24] = 32;                         /* bitmap scanline unit */
    p[25] = 32;                         /* bitmap scanline pad */
    p[26] = 8;                          /* min keycode */
    p[27] = 255;                        /* max keycode */
    p += 32;
    memcpy(p, VENDOR, sizeof VENDOR - 1);
    p += pad4(sizeof VENDOR - 1);
    const uint8_t formats[2][3] = {{1, 1, 32}, {24, 32, 32}};
    for (int i = 0; i < 2; i++, p += 8) {
        memcpy(p, formats[i], 3);
    }
    put32(p, ROOT_WINDOW);
    put32(p + 4, ROOT_COLORMAP);
    put32(p + 8, 0xffffff);             /* white pixel */
    put32(p + 12, 0);                   /* black pixel */
    put16(p + 20, WIDTH);
    put16(p + 22, HEIGHT);
    put16(p + 24, 170);                 /* millimetres */
    put16(p + 26, 127);
    put16(p + 28, 1);                   /* installed colormaps */
    put16(p + 30, 1);
    put32(p + 32, ROOT_VISUAL);
    p[38] = 24;                         /* root depth */
    p[39] = 1;                          /* depths */
    p += 40;
    p[0] = 24;
    put16(p + 2, 1);                    /* visuals */
    p += 8;
    put32(p, ROOT_VISUAL);
    p[4] = 4;                           /* TrueColor */
    p[5] = 8;                           /* bits per RGB value */
    put16(p + 6, 256);
    put32(p + 8, 0xff0000);
    put32(p + 12, 0x00ff00);
    put32(p + 16, 0x0000ff);
    p += 24;
    size_t len = (size_t)(p - r);
    r[0] = 1;                           /* success */
    put16(r + 2, 11);                   /* protocol 11.0 */
    put16(r + 6, (uint16_t)((len - 8) / 4));
    return len;
}

/* What the server saw of the client's window. */
struct seen {
    int created, mapped, named;
};

static int serve(int fd, struct seen *seen) {
    uint8_t req[12];
    if (read_full(fd, req, sizeof req) != 1 || req[0] != 'l') {
        fprintf(stderr, "x11: no little-endian setup request\n");
        return -1;
    }
    size_t auth = pad4(get16(req + 6)) + pad4(get16(req + 8));
    uint8_t buf[65536];
    if (auth > sizeof buf || (auth && read_full(fd, buf, auth) != 1)) {
        return -1;
    }
    uint8_t setup[256];
    if (write_full(fd, setup, setup_reply(setup)) < 0) {
        return -1;
    }
    uint16_t seq = 0;
    uint32_t next_atom = FIRST_ATOM;
    uint32_t window = 0;
    for (;;) {
        int rc = read_full(fd, buf, 4);
        if (rc == 0) {
            return 0; /* the client closed the display */
        }
        size_t len = (size_t)get16(buf + 2) * 4;
        if (rc < 0 || len < 4 || len > sizeof buf || read_full(fd, buf + 4, len - 4) != 1) {
            fprintf(stderr, "x11: bad request (opcode %u)\n", buf[0]);
            return -1;
        }
        seq++;
        uint8_t reply[32] = {1};
        int answer = 1;
        switch (buf[0]) {
        case 1: /* CreateWindow */
            window = get32(buf + 4);
            seen->created = get16(buf + 16) == 100 && get16(buf + 18) == 60;
            answer = 0;
            break;
        case 8: /* MapWindow */
            seen->mapped = window && get32(buf + 4) == window;
            answer = 0;
            break;
        case 16: /* InternAtom */
            put32(reply + 8, next_atom++);
            break;
        case 18: /* ChangeProperty */
            if (get32(buf + 8) == XA_WM_NAME && get32(buf + 20) == 9
                && memcmp(buf + 24, "x11-smoke", 9) == 0) {
                seen->named = 1;
            }
            answer = 0;
            break;
        case 20: /* GetProperty: none set */
        case 98: /* QueryExtension: none present */
            break;
        case 43: /* GetInputFocus */
            reply[1] = 1;
            put32(reply + 8, ROOT_WINDOW);
            break;
        default:
            answer = 0;
            break;
        }
        if (answer) {
            put16(reply + 2, seq);
            if (write_full(fd, reply, sizeof reply) < 0) {
                return -1;
            }
        }
    }
}

/* ---- the client ---- */

static int client(void) {
    setenv("DISPLAY", ":5", 1);
    Display *d = XOpenDisplay(NULL);
    if (!d) {
        printf("[ FAIL ] x11 XOpenDisplay\n");
        return 1;
    }
    int s = DefaultScreen(d);
    if (DisplayWidth(d, s) != WIDTH || DisplayHeight(d, s) != HEIGHT || DefaultDepth(d, s) != 24
        || RootWindow(d, s) != ROOT_WINDOW || strcmp(ServerVendor(d), VENDOR) != 0
        || ProtocolVersion(d) != 11) {
        printf("[ FAIL ] x11 screen %dx%d depth %d root 0x%lx vendor \"%s\"\n",
               DisplayWidth(d, s), DisplayHeight(d, s), DefaultDepth(d, s),
               (unsigned long)RootWindow(d, s), ServerVendor(d));
        return 1;
    }
    Window w = XCreateSimpleWindow(d, RootWindow(d, s), 10, 20, 100, 60, 1,
                                   BlackPixel(d, s), WhitePixel(d, s));
    XStoreName(d, w, "x11-smoke");
    XMapWindow(d, w);
    Atom a = XInternAtom(d, "X11_SMOKE", False);
    if (a != FIRST_ATOM) {
        printf("[ FAIL ] x11 InternAtom gave %lu\n", (unsigned long)a);
        return 1;
    }
    XSync(d, False);
    XCloseDisplay(d);
    return 0;
}

int main(void) {
    int l = socket(AF_UNIX, SOCK_STREAM, 0);
    struct sockaddr_un addr = {.sun_family = AF_UNIX};
    strcpy(addr.sun_path, SOCKET);
    if (l < 0 || bind(l, (struct sockaddr *)&addr, sizeof addr) < 0 || listen(l, 1) < 0) {
        printf("[ FAIL ] x11 listen on %s (errno %d)\n", SOCKET, errno);
        return 1;
    }
    pid_t pid = fork();
    if (pid == 0) {
        close(l);
        _exit(client());
    }
    int fd = accept(l, NULL, NULL);
    if (fd < 0) {
        printf("[ FAIL ] x11 accept (errno %d)\n", errno);
        return 1;
    }
    struct seen seen = {0};
    int served = serve(fd, &seen);
    close(fd);
    close(l);
    int status = 0;
    waitpid(pid, &status, 0);
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        return 1; /* the client said why */
    }
    if (served < 0 || !seen.created || !seen.named || !seen.mapped) {
        printf("[ FAIL ] x11 server saw: created %d, named %d, mapped %d (served %d)\n",
               seen.created, seen.named, seen.mapped, served);
        return 1;
    }
    printf("[ OK ] x11\n");
    return 0;
}
