#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <signal.h>
#include <unistd.h>
#include <sys/time.h>

#include "myos_syscalls.h"

void sync(void) {
}

char *ttyname(int fd) {
    static char name[MYOS_TTY_PATH];
    int master;

    if (myos_tty_dir(fd, name, sizeof name - 5, &master) != 0 || master) {
        errno = ENOTTY;
        return NULL;
    }
    strcat(name, "/data");
    return name;
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
