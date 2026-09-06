#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <signal.h>
#include <unistd.h>

#include "myos_syscalls.h"

/* Kernel ABI: three usizes {handler, flags, mask}. handler 0=DFL, 1=IGN. */
struct myos_ksigaction {
    unsigned long handler;
    unsigned long flags;
    unsigned long mask;
};

int sigaction(int sig, const struct sigaction *restrict act,
    struct sigaction *restrict oact) {
    struct myos_ksigaction kin;
    struct myos_ksigaction kout;
    long ret;

    if (sig <= 0 || sig > 31) {
        errno = EINVAL;
        return -1;
    }
    memset(&kin, 0, sizeof(kin));
    memset(&kout, 0, sizeof(kout));
    if (act != NULL) {
        kin.handler = (unsigned long)(uintptr_t)act->sa_handler;
        kin.flags = (unsigned long)act->sa_flags;
    }
    ret = myos_syscall3(
        MYOS_SYS_SIGACTION,
        (long)sig,
        act ? (long)(uintptr_t)&kin : 0,
        oact ? (long)(uintptr_t)&kout : 0);
    if (ret == (long)MYOS_SYSERR) {
        /* Custom handlers / ignore of SIGKILL → ENOSYS. */
        errno = ENOSYS;
        return -1;
    }
    if (oact != NULL) {
        memset(oact, 0, sizeof(*oact));
        oact->sa_handler = (_sig_func_ptr)(uintptr_t)kout.handler;
        oact->sa_flags = (int)kout.flags;
    }
    return 0;
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
