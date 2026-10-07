/* fb-smoke: boot-CI guest test for /dev/fb (modules/console/src/fbdev.rs,
 * docs/fb.md).
 *
 * Reads the geometry line from ctl, takes the screen (`graphics`), maps
 * data MAP_SHARED and checks that the mapping is the framebuffer itself:
 * what is drawn through it reads back through read(), what write() puts
 * there shows in it, and a forked child draws into the same pixels. A
 * MAP_PRIVATE mapping is a copy. Then the screen goes back to text by a
 * `text` write, by closing ctl, and by the exit of a child holding it.
 * Prints [ OK ] fb.
 *
 * `fb_smoke text`: the text console's UTF-8 (modules/console/src/fb.rs,
 * font.rs, docs/tty.md). Draws characters at the top left through
 * /dev/console/data and reads the two cells back from /dev/fb/data: a
 * box-drawing, block or braille character is drawn, in one cell; one
 * without a glyph is a single '?', a wide one two cells, a combining mark
 * none; xterm's 256 colors and RGB are colors. Prints [ OK ] fb text.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define CTL "/dev/fb/ctl"
#define DATA "/dev/fb/data"
#define PIXELS 64

struct geom {
    unsigned width, height, depth, pitch;
    char chan[32];
    char mode[16];
};

static int fail(const char *what) {
    printf("[ FAIL ] fb %s (errno %d)\n", what, errno);
    return 1;
}

/* The ctl line: width height depth chan pitch mode. */
static int geometry(struct geom *g) {
    char line[128];
    int fd = open(CTL, O_RDONLY);
    ssize_t n = fd < 0 ? -1 : read(fd, line, sizeof line - 1);
    if (fd >= 0) {
        close(fd);
    }
    if (n <= 0) {
        return -1;
    }
    line[n] = '\0';
    printf("[ INFO ] fb ctl: %s", line);
    return sscanf(line, "%u %u %u %31s %u %15s", &g->width, &g->height, &g->depth, g->chan,
               &g->pitch, g->mode) == 6 ? 0 : -1;
}

/* Bits the chan word describes (`x8r8g8b8` is 32). */
static unsigned chan_bits(const char *c) {
    unsigned bits = 0;
    while (*c) {
        if (strchr("xrgb", *c) == NULL) {
            return 0;
        }
        bits += (unsigned)strtoul(c + 1, (char **)&c, 10);
    }
    return bits;
}

static int mode_is(const char *mode) {
    struct geom g;
    return geometry(&g) == 0 && strcmp(g.mode, mode) == 0;
}

static int ctl_write(int fd, const char *cmd) {
    return write(fd, cmd, strlen(cmd)) == (ssize_t)strlen(cmd) ? 0 : -1;
}

static int read_at(int fd, off_t off, void *buf, size_t n) {
    return lseek(fd, off, SEEK_SET) == off && read(fd, buf, n) == (ssize_t)n ? 0 : -1;
}

/* The text console's first two cells (8x8 pixels each) after drawing `s`
 * on a cleared screen; 0 when they could be read. */
#define CELLS_W 16
static int cells(int con, int fb, unsigned pitch, const char *s, uint32_t out[8][CELLS_W]) {
    char buf[64];
    int n = snprintf(buf, sizeof buf, "\033[H\033[J%s\033[10;1H", s);
    if (write(con, buf, (size_t)n) != n) {
        return -1;
    }
    for (int y = 0; y < 8; y++) {
        if (read_at(fb, (off_t)y * pitch, out[y], sizeof out[y]) < 0) {
            return -1;
        }
    }
    return 0;
}

static int text_main(void) {
    struct geom g;
    if (geometry(&g) < 0 || g.depth != 32 || g.width < CELLS_W) {
        return fail("text: ctl");
    }
    /* The runner turned the screen copy off (run.sh): on for the check. */
    char line[512];
    int ctl = open("/dev/console/ctl", O_RDWR);
    ssize_t n = ctl < 0 ? -1 : read(ctl, line, sizeof line - 1);
    if (n <= 0) {
        return fail("text: console ctl");
    }
    line[n] = '\0';
    char *mirror = strstr(line, "mirror ");
    char *end = mirror ? strchr(mirror, '\n') : NULL;
    if (end == NULL || ctl_write(ctl, "mirror on\n") < 0) {
        return fail("text: mirror on");
    }
    end[1] = '\0';
    int con = open("/dev/console/data", O_WRONLY);
    int fb = open(DATA, O_RDONLY);
    if (con < 0 || fb < 0) {
        return fail("text: open");
    }

    static const struct {
        const char *name, *s;
    } samples[] = {
        {"blank", ""},
        {"?", "?"},
        {"??", "??"},
        {"line", "\u2500"},     /* ─ */
        {"half", "\u2584"},     /* ▄ */
        {"dots", "\u28ff"},     /* ⣿ */
        {"e-acute", "\u00e9"},  /* é: no glyph */
        {"wide", "\u4e2d"},     /* 中 */
        {"e", "e"},
        {"e+comb", "e\u0301"},  /* e, combining acute */
        {"block", "\u2588"},    /* █ */
        {"256", "\033[38;5;196m\u2588\033[m"},
        {"rgb", "\033[38;2;255;0;0m\u2588\033[m"},
    };
    enum { BLANK, Q, QQ, LINE, HALF, DOTS, E_ACUTE, WIDE, E, E_COMB, BLOCK, C256, RGB, SAMPLES };
    static uint32_t px[SAMPLES][8][CELLS_W];
    for (int i = 0; i < SAMPLES; i++) {
        if (cells(con, fb, g.pitch, samples[i].s, px[i]) < 0) {
            return fail("text: draw");
        }
    }
    write(con, "\033[H\033[J\n", 7);
    ctl_write(ctl, mirror);
    close(ctl);

#define SAME(a, b) (memcmp(px[a], px[b], sizeof px[a]) == 0)
    const char *bad = NULL;
    for (int i = LINE; i <= DOTS && bad == NULL; i++) {
        if (SAME(i, BLANK) || SAME(i, Q) || SAME(i, QQ)) {
            bad = samples[i].name;
        }
    }
    if (bad == NULL && (SAME(LINE, HALF) || SAME(HALF, DOTS))) {
        bad = "line, half and dots alike";
    }
    if (bad == NULL && !SAME(E_ACUTE, Q)) {
        bad = "e-acute is not one '?'";
    }
    if (bad == NULL && !SAME(WIDE, QQ)) {
        bad = "wide is not two cells";
    }
    if (bad == NULL && !SAME(E_COMB, E)) {
        bad = "the combining mark took a cell";
    }
    if (bad == NULL && (!SAME(C256, RGB) || SAME(C256, BLOCK))) {
        bad = "256 colors / RGB";
    }
    if (bad != NULL) {
        printf("[ FAIL ] fb text: %s\n", bad);
        return 1;
    }
    printf("[ OK ] fb text\n");
    return 0;
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "text") == 0) {
        return text_main();
    }
    struct geom g;
    struct stat st;
    uint32_t want[PIXELS], got[PIXELS];

    if (geometry(&g) < 0) {
        return fail("ctl");
    }
    if (g.depth != 32 || g.width < PIXELS || g.height < 3 || g.pitch < g.width * 4
        || chan_bits(g.chan) != g.depth || strcmp(g.mode, "text") != 0) {
        return fail("ctl values");
    }
    size_t len = (size_t)g.pitch * g.height;
    if (stat(DATA, &st) < 0 || (size_t)st.st_size != len) {
        return fail("data size");
    }

    int ctl = open(CTL, O_RDWR);
    int fd = open(DATA, O_RDWR);
    if (ctl < 0 || fd < 0) {
        return fail("open");
    }
    if (ctl_write(ctl, "graphics") < 0 || !mode_is("graphics")) {
        return fail("graphics");
    }
    uint8_t *fb = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (fb == MAP_FAILED) {
        return fail("mmap");
    }

    /* Row 0: drawn through the mapping, read back with read(). */
    for (int i = 0; i < PIXELS; i++) {
        want[i] = 0x00ff0000u | (uint32_t)i;
    }
    memcpy(fb, want, sizeof want);
    if (read_at(fd, 0, got, sizeof got) < 0 || memcmp(want, got, sizeof want) != 0) {
        return fail("mmap -> read");
    }
    /* Row 1: put there with write(), seen in the mapping. */
    for (int i = 0; i < PIXELS; i++) {
        want[i] = 0x0000ff00u | (uint32_t)i;
    }
    if (lseek(fd, g.pitch, SEEK_SET) != (off_t)g.pitch
        || write(fd, want, sizeof want) != (ssize_t)sizeof want
        || memcmp(fb + g.pitch, want, sizeof want) != 0) {
        return fail("write -> mmap");
    }
    /* Row 2: a forked child shares the mapping. */
    pid_t pid = fork();
    if (pid == 0) {
        memset(fb + 2 * g.pitch, 0xff, sizeof want);
        _exit(0);
    }
    int status;
    if (pid < 0 || waitpid(pid, &status, 0) != pid) {
        return fail("fork");
    }
    memset(want, 0xff, sizeof want);
    if (memcmp(fb + 2 * g.pitch, want, sizeof want) != 0) {
        return fail("fork shares the mapping");
    }
    /* MAP_PRIVATE: a copy of the pixels, not the framebuffer. */
    uint8_t *copy = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
    if (copy == MAP_FAILED || memcmp(copy, fb, 4096) != 0) {
        return fail("MAP_PRIVATE copy");
    }
    memset(copy, 0, 64);
    if (memcmp(fb, copy, 64) == 0) {
        return fail("MAP_PRIVATE wrote through");
    }
    if (munmap(copy, 4096) < 0 || munmap(fb, len) < 0) {
        return fail("munmap");
    }
    close(fd);

    /* Back to text: by a write, by the last close of ctl, by an exit. */
    if (ctl_write(ctl, "text") < 0 || !mode_is("text")) {
        return fail("text");
    }
    if (ctl_write(ctl, "graphics") < 0) {
        return fail("graphics again");
    }
    close(ctl);
    if (!mode_is("text")) {
        return fail("text after closing ctl");
    }
    pid = fork();
    if (pid == 0) {
        int c = open(CTL, O_WRONLY);
        _exit(c < 0 || ctl_write(c, "graphics") < 0 || !mode_is("graphics") ? 1 : 0);
    }
    if (pid < 0 || waitpid(pid, &status, 0) != pid || !WIFEXITED(status)
        || WEXITSTATUS(status) != 0) {
        return fail("child graphics");
    }
    if (!mode_is("text")) {
        return fail("text after the holder's exit");
    }
    printf("[ OK ] fb\n");
    return 0;
}
