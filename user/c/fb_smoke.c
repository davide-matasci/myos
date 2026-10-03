/* fb-smoke: boot-CI guest test for /dev/fb0 (kernel/src/fb.rs, docs/fb.md).
 *
 * Reads the geometry (FBIOGET_VSCREENINFO / FBIOGET_FSCREENINFO), takes the
 * screen (KDSETMODE KD_GRAPHICS), maps the framebuffer MAP_SHARED and checks
 * that the mapping is the framebuffer itself: what is drawn through it reads
 * back through read(), what write() puts there shows in it, and a forked
 * child draws into the same pixels. Then gives the screen back, and checks
 * that a child exiting in graphics mode does too. Prints [ OK ] fb.
 */
#include <errno.h>
#include <fcntl.h>
#include <linux/fb.h>
#include <linux/kd.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#define PIXELS 64

static int fail(const char *what) {
    printf("[ FAIL ] fb %s (errno %d)\n", what, errno);
    return 1;
}

static int mode(int fd) {
    int m = -1;
    if (ioctl(fd, KDGETMODE, &m) < 0) {
        return -1;
    }
    return m;
}

static int read_at(int fd, off_t off, void *buf, size_t n) {
    return lseek(fd, off, SEEK_SET) == off && read(fd, buf, n) == (ssize_t)n ? 0 : -1;
}

int main(void) {
    struct fb_var_screeninfo var;
    struct fb_fix_screeninfo fix;
    uint32_t want[PIXELS], got[PIXELS];
    int fd = open("/dev/fb0", O_RDWR);
    if (fd < 0) {
        return fail("open");
    }
    if (ioctl(fd, FBIOGET_VSCREENINFO, &var) < 0 || ioctl(fd, FBIOGET_FSCREENINFO, &fix) < 0) {
        return fail("geometry");
    }
    printf("[ INFO ] fb %ux%u %ubpp pitch %u size %u\n", var.xres, var.yres,
        var.bits_per_pixel, fix.line_length, fix.smem_len);
    if (var.bits_per_pixel != 32 || var.xres < PIXELS || var.yres < 3
        || fix.line_length < var.xres * 4 || fix.smem_len != fix.line_length * var.yres) {
        return fail("geometry values");
    }

    if (ioctl(fd, KDSETMODE, KD_GRAPHICS) < 0 || mode(fd) != KD_GRAPHICS) {
        return fail("KD_GRAPHICS");
    }
    uint8_t *fb = mmap(NULL, fix.smem_len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (fb == MAP_FAILED) {
        return fail("mmap");
    }

    /* Row 0: drawn through the mapping, read back with read(). */
    for (int i = 0; i < PIXELS; i++) {
        want[i] = (uint32_t)(i * 4) << var.red.offset | 0xffu << var.blue.offset;
    }
    memcpy(fb, want, sizeof want);
    if (read_at(fd, 0, got, sizeof got) < 0 || memcmp(want, got, sizeof want) != 0) {
        return fail("mmap -> read");
    }
    /* Row 1: put there with write(), seen in the mapping. */
    for (int i = 0; i < PIXELS; i++) {
        want[i] = 0xffu << var.green.offset | (uint32_t)i;
    }
    if (lseek(fd, fix.line_length, SEEK_SET) != fix.line_length
        || write(fd, want, sizeof want) != (ssize_t)sizeof want
        || memcmp(fb + fix.line_length, want, sizeof want) != 0) {
        return fail("write -> mmap");
    }
    /* Row 2: a forked child shares the mapping. */
    pid_t pid = fork();
    if (pid == 0) {
        memset(fb + 2 * fix.line_length, 0xff, sizeof want);
        _exit(0);
    }
    int status;
    if (pid < 0 || waitpid(pid, &status, 0) != pid) {
        return fail("fork");
    }
    memset(want, 0xff, sizeof want);
    if (memcmp(fb + 2 * fix.line_length, want, sizeof want) != 0) {
        return fail("fork shares the mapping");
    }

    if (munmap(fb, fix.smem_len) < 0) {
        return fail("munmap");
    }
    if (ioctl(fd, KDSETMODE, KD_TEXT) < 0 || mode(fd) != KD_TEXT) {
        return fail("KD_TEXT");
    }
    /* A program that dies in graphics mode hands the screen back. */
    pid = fork();
    if (pid == 0) {
        _exit(ioctl(fd, KDSETMODE, KD_GRAPHICS) < 0 ? 1 : 0);
    }
    if (pid < 0 || waitpid(pid, &status, 0) != pid || !WIFEXITED(status)
        || WEXITSTATUS(status) != 0) {
        return fail("child KD_GRAPHICS");
    }
    if (mode(fd) != KD_TEXT) {
        return fail("text mode after the owner's exit");
    }
    close(fd);
    printf("[ OK ] fb\n");
    return 0;
}
