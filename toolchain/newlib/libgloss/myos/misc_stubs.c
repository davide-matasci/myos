#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <signal.h>
#include <unistd.h>
#include <sys/time.h>

#include "myos_syscalls.h"

/* Kernel ABI: three usizes {handler, flags, mask}. handler 0=DFL, 1=IGN. */
struct myos_ksigaction {
    unsigned long handler;
    unsigned long flags;
    unsigned long mask;
};

/* The kernel has no userspace handler trampolines, so a custom SIGCHLD handler
 * is kept here and invoked by myos_sigchld_dispatch() (called from select()/
 * poll()) when the kernel reports the pending bit through SIGCHLD_TAKE. */
static _sig_func_ptr myos_sigchld_handler;

/* fd the SIGCHLD handler wrote to (its self-pipe write end), captured via _write
 * while insig is set. Used by poll()/select() to force the matching read end
 * readable so a server's select() returns >0 and it reaps the child. */
volatile long myos_sigchld_wfd = -1;
volatile int myos_sigchld_insig = 0;

/* 1 if a custom SIGCHLD handler is installed. poll()/select() use this to cap
 * their blocking timeout so the dispatch below runs periodically: myos has no
 * signal trampolines and a blocked select() is not interrupted by a child exit. */
int myos_sigchld_armed(void) {
    return myos_sigchld_handler != NULL && myos_sigchld_handler != SIG_DFL &&
           myos_sigchld_handler != SIG_IGN;
}

void myos_sigchld_dispatch(void) {
    if (!myos_sigchld_armed()) {
        return;
    }
    long r = myos_syscall0(MYOS_SYS_SIGCHLD_TAKE);
    if (r != 1) {
        r = myos_syscall0(MYOS_SYS_SIGCHLD_PENDING);
    }
        if (r == 1) {
        /* Level-triggered: while a zombie child exists, keep invoking the
         * handler so its signal_pipe write is visible to dropbear's select()
         * (myos select is not woken by the child exit itself). */

        myos_sigchld_wfd = -1; /* capture the handler's FIRST write (self-pipe) */
        myos_sigchld_insig = 1;
        myos_sigchld_handler(SIGCHLD);
        myos_sigchld_insig = 0;
    }
}

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
        /* The kernel does not support custom handler trampolines yet (DFL/IGN
         * only). Real programs (dropbear) install handlers for SIGCHLD reaping
         * and SIGINT/SIGTERM graceful shutdown; degrade to SIG_IGN instead of
         * failing the whole program. SIG_IGN for SIGCHLD is POSIX-correct
         * (children are auto-reaped, no zombies). SIGSEGV keeps default
         * disposition (crash normally) — ignoring it would loop. */
        int custom = act != NULL && kin.handler != 0 /* DFL */ &&
            kin.handler != 1 /* IGN */;
        /* SIGCHLD: no trampoline exists, but the kernel raises a pending bit
         * on child exit that select()/poll() picks up (myos_sigchld_dispatch).
         * Keep the handler instead of degrading to SIG_IGN, or a server like
         * dropbear never reaps children and never sends an SSH exit-status. */
        if (custom && sig == SIGCHLD) {
            myos_sigchld_handler = (_sig_func_ptr)(uintptr_t)kin.handler;
            if (oact != NULL) {
                memset(oact, 0, sizeof(*oact));
                oact->sa_handler = (_sig_func_ptr)(uintptr_t)kin.handler;
                oact->sa_flags = (int)kin.flags;
            }
            return 0;
        }
        if (custom && sig != SIGSEGV && sig != SIGKILL) {
            kin.handler = 1;
            kin.flags = 0;
            ret = myos_syscall3(
                MYOS_SYS_SIGACTION,
                (long)sig,
                (long)(uintptr_t)&kin,
                0);
            if (ret != (long)MYOS_SYSERR) {
                if (oact != NULL) {
                    memset(oact, 0, sizeof(*oact));
                    oact->sa_handler = (_sig_func_ptr)(uintptr_t)kin.handler;
                    oact->sa_flags = (int)kin.flags;
                }
                return 0;
            }
        }
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
