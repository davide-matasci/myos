/* myos libgloss: the terminal behind an fd, through its files (docs/tty.md).
 *
 * A terminal is a directory: `data` is the terminal, `ctl` its state as
 * text (the termios fields, the control characters, the window size, one
 * per line) and the commands `ctty` and `flush`. An fd leads to it through
 * its /proc/self/fd link, whose target ends in /data (or /master for a
 * pty's master end). tcgetattr, tcsetattr, isatty, ttyname, openpty and
 * the ioctl() shim are all built on these. */
#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>

#include "myos_syscalls.h"

/* Numbers are written by hand: snprintf would pull newlib's floating point
 * formatting into every program that calls isatty (newlib's stdio does at
 * its first output), and on riscv64 that needs the soft-float helpers. */
static char *put_num(char *p, unsigned long v, unsigned base, int min_digits) {
    char digits[24];
    int n = 0;
    do {
        digits[n++] = "0123456789abcdef"[v % base];
        v /= base;
    } while (v != 0 || n < min_digits);
    while (n > 0) {
        *p++ = digits[--n];
    }
    *p = '\0';
    return p;
}

static char *put_str(char *p, const char *s) {
    size_t n = strlen(s);
    memcpy(p, s, n + 1);
    return p + n;
}

int myos_tty_dir(int fd, char *dir, size_t cap, int *master) {
    char link[32];
    char target[MYOS_TTY_PATH];
    const char *end;
    ssize_t n;
    size_t len;
    int saved = errno;

    if (fd < 0) {
        errno = EBADF;
        return -1;
    }
    put_num(put_str(link, "/proc/self/fd/"), (unsigned long)fd, 10, 1);
    n = readlink(link, target, sizeof target - 1);
    if (n < 0) {
        errno = ENOTTY;
        return -1;
    }
    target[n] = '\0';
    end = strrchr(target, '/');
    if (strncmp(target, "/dev/", 5) != 0 || end == NULL
        || (strcmp(end, "/data") != 0 && strcmp(end, "/master") != 0)) {
        errno = ENOTTY;
        return -1;
    }
    len = (size_t)(end - target);
    if (dir != NULL) {
        if (len >= cap) {
            errno = ENAMETOOLONG;
            return -1;
        }
        memcpy(dir, target, len);
        dir[len] = '\0';
    }
    if (master != NULL) {
        *master = strcmp(end, "/master") == 0;
    }
    errno = saved;
    return 0;
}

/* The ctl text of the terminal `fd` is on, NUL-terminated in `buf`. */
static int ctl_read(int fd, char *buf, size_t cap) {
    char path[MYOS_TTY_PATH];
    size_t have = 0;
    int cfd;

    if (myos_tty_dir(fd, path, sizeof path - 4, NULL) != 0) {
        return -1;
    }
    strcat(path, "/ctl");
    cfd = open(path, O_RDONLY);
    if (cfd < 0) {
        errno = ENOTTY;
        return -1;
    }
    while (have < cap - 1) {
        ssize_t n = read(cfd, buf + have, cap - 1 - have);
        if (n <= 0) {
            break;
        }
        have += (size_t)n;
    }
    close(cfd);
    buf[have] = '\0';
    return 0;
}

int myos_tty_write(int fd, const char *text) {
    char path[MYOS_TTY_PATH];
    size_t len = strlen(text);
    int cfd;
    ssize_t n;

    if (myos_tty_dir(fd, path, sizeof path - 4, NULL) != 0) {
        return -1;
    }
    strcat(path, "/ctl");
    cfd = open(path, O_WRONLY);
    if (cfd < 0) {
        errno = ENOTTY;
        return -1;
    }
    n = write(cfd, text, len);
    close(cfd);
    if (n != (ssize_t)len) {
        /* The kernel refused the text: nothing of it was applied. */
        errno = EINVAL;
        return -1;
    }
    return 0;
}

int myos_tty_get(int fd, struct termios *t, struct winsize *w) {
    char text[MYOS_TTY_CTL];
    char *line;
    char *next;

    if (ctl_read(fd, text, sizeof text) != 0) {
        return -1;
    }
    if (t != NULL) {
        memset(t, 0, sizeof *t);
    }
    if (w != NULL) {
        memset(w, 0, sizeof *w);
    }
    for (line = text; *line != '\0'; line = next) {
        char *word = strchr(line, '\n');
        if (word != NULL) {
            *word = '\0';
            next = word + 1;
        } else {
            next = line + strlen(line);
        }
        word = strchr(line, ' ');
        if (word == NULL) {
            continue;
        }
        *word++ = '\0';
        if (t != NULL && strcmp(line, "iflag") == 0) {
            t->c_iflag = (tcflag_t)strtoul(word, NULL, 0);
        } else if (t != NULL && strcmp(line, "oflag") == 0) {
            t->c_oflag = (tcflag_t)strtoul(word, NULL, 0);
        } else if (t != NULL && strcmp(line, "cflag") == 0) {
            t->c_cflag = (tcflag_t)strtoul(word, NULL, 0);
        } else if (t != NULL && strcmp(line, "lflag") == 0) {
            t->c_lflag = (tcflag_t)strtoul(word, NULL, 0);
        } else if (t != NULL && strcmp(line, "cc") == 0) {
            int i;
            for (i = 0; i < NCCS && *word != '\0'; i++) {
                char *rest;
                t->c_cc[i] = (cc_t)strtoul(word, &rest, 16);
                word = rest + (*rest == ' ');
            }
        } else if (t != NULL && strcmp(line, "speed") == 0) {
            char *rest;
            t->c_ispeed = (speed_t)strtoul(word, &rest, 0);
            t->c_ospeed = (speed_t)strtoul(rest, NULL, 0);
        } else if (w != NULL && strcmp(line, "winsize") == 0) {
            char *rest;
            w->ws_row = (unsigned short)strtoul(word, &rest, 0);
            w->ws_col = (unsigned short)strtoul(rest, NULL, 0);
        }
    }
    return 0;
}

int myos_tty_set(int fd, const struct termios *t) {
    char text[MYOS_TTY_CTL];
    char *p = text;
    int i;

    p = put_num(put_str(p, "iflag 0x"), t->c_iflag, 16, 1);
    p = put_num(put_str(p, "\noflag 0x"), t->c_oflag, 16, 1);
    p = put_num(put_str(p, "\ncflag 0x"), t->c_cflag, 16, 1);
    p = put_num(put_str(p, "\nlflag 0x"), t->c_lflag, 16, 1);
    p = put_str(p, "\ncc");
    for (i = 0; i < NCCS; i++) {
        p = put_num(put_str(p, " "), t->c_cc[i], 16, 2);
    }
    p = put_num(put_str(p, "\nspeed "), t->c_ispeed, 10, 1);
    p = put_num(put_str(p, " "), t->c_ospeed, 10, 1);
    put_str(p, "\n");
    return myos_tty_write(fd, text);
}

int myos_tty_set_winsize(int fd, unsigned rows, unsigned cols) {
    char line[48];
    char *p = put_num(put_str(line, "winsize "), rows, 10, 1);
    p = put_num(put_str(p, " "), cols, 10, 1);
    put_str(p, "\n");
    return myos_tty_write(fd, line);
}
