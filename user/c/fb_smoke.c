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

int main(void) {
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
