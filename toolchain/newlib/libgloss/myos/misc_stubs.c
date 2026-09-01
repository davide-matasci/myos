#include <errno.h>
#include <stddef.h>
#include <signal.h>
#include <unistd.h>

int sigaction(int sig, const struct sigaction *restrict act,
    struct sigaction *restrict oact) {
    (void)sig;
    (void)act;
    (void)oact;
    errno = ENOSYS;
    return -1;
}

void sync(void) {
}

char *ttyname(int fd) {
    if (isatty(fd)) {
        return "/dev/console";
    }
    errno = ENOTTY;
    return NULL;
}

char *getpass(const char *prompt) {
    static char buf[128];
    size_t i = 0;

    if (prompt != NULL) {
        const char *p = prompt;
        while (*p) {
            write(2, p, 1);
            p++;
        }
    }
    for (;;) {
        char c = 0;
        ssize_t n = read(0, &c, 1);
        if (n <= 0) {
            break;
        }
        if (c == '\n' || c == '\r') {
            break;
        }
        if (i + 1 < sizeof(buf)) {
            buf[i++] = c;
        }
    }
    buf[i] = '\0';
    write(2, "\n", 1);
    return buf;
}
