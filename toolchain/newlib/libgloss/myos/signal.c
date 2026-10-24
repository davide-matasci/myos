/*
 * Signal handlers for newlib: sigaction(), signal(), the mask calls and
 * the handler dispatch, over the kernel's records (toolchain/libmyos/
 * sigtramp.c: the trampoline, the 32-bit masks, struct myos_ksigaction).
 * newlib's sigset_t is one unsigned long, and its siginfo_t is what the
 * kernel writes into the frame (int si_signo; int si_code; union sigval
 * si_value; void *si_addr).
 */

#include <errno.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "myos_syscalls.h"

#define MYOS_SIGFRAME_SIGNO 1
#define MYOS_SIGFRAME_HANDLER 2
#define MYOS_SIGFRAME_SIGINFO 10

void __myos_sigdispatch(unsigned long *frame) {
    void (*handler)(int, siginfo_t *, void *) =
        (void (*)(int, siginfo_t *, void *))(uintptr_t)frame[MYOS_SIGFRAME_HANDLER];
    __myos_sig_count++;
    /* A plain sa_handler takes one argument; passing three is harmless. */
    handler((int)frame[MYOS_SIGFRAME_SIGNO],
            (siginfo_t *)&frame[MYOS_SIGFRAME_SIGINFO], NULL);
}

int sigaction(int sig, const struct sigaction *restrict act,
    struct sigaction *restrict oact) {
    struct myos_ksigaction kin;
    struct myos_ksigaction kout;

    memset(&kin, 0, sizeof(kin));
    memset(&kout, 0, sizeof(kout));
    if (act != NULL) {
        kin.handler = (unsigned long)(uintptr_t)act->sa_handler;
        kin.flags = (unsigned long)(unsigned int)act->sa_flags;
        kin.mask = (unsigned long)act->sa_mask;
    }
    if (__myos_sigaction_k(sig, act ? &kin : NULL, oact ? &kout : NULL) != 0) {
        return -1;
    }
    if (oact != NULL) {
        memset(oact, 0, sizeof(*oact));
        oact->sa_handler = (_sig_func_ptr)(uintptr_t)kout.handler;
        oact->sa_flags = (int)kout.flags;
        oact->sa_mask = (sigset_t)kout.mask;
    }
    return 0;
}

/* BSD semantics, like glibc: the handler stays installed and interrupted
 * syscalls restart. */
_sig_func_ptr signal(int sig, _sig_func_ptr func) {
    struct sigaction sa;
    struct sigaction old;

    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = func;
    sa.sa_flags = SA_RESTART;
    if (sigaction(sig, &sa, &old) != 0) {
        return SIG_ERR;
    }
    return old.sa_handler;
}

/* The kernel reads and writes the mask as 32 bits; sigset_t is one
 * unsigned long (sys/_sigset.h), so go through 32-bit temporaries or the
 * upper half of *oset keeps whatever was there. */
int sigprocmask(int how, const sigset_t *restrict set, sigset_t *restrict oset) {
    unsigned int kset = 0;
    unsigned int kold = 0;

    if (set != NULL) {
        kset = (unsigned int)*set;
    }
    if (__myos_sigprocmask_k(how, set ? &kset : NULL, oset ? &kold : NULL) != 0) {
        return -1;
    }
    if (oset != NULL) {
        *oset = (sigset_t)kold;
    }
    return 0;
}

int sigpending(sigset_t *set) {
    if (set == NULL) {
        errno = EFAULT;
        return -1;
    }
    *set = (sigset_t)__myos_sigpending_k();
    return 0;
}

int sigsuspend(const sigset_t *mask) {
    if (mask == NULL) {
        errno = EFAULT;
        return -1;
    }
    return __myos_sigsuspend_k((unsigned int)*mask);
}

int sigwait(const sigset_t *restrict set, int *restrict sig) {
    int r;

    if (set == NULL || sig == NULL) {
        return EINVAL;
    }
    r = __myos_sigwait_k((unsigned int)*set);
    if (r < 0) {
        return EINVAL;
    }
    *sig = r;
    return 0;
}

/* Processes have a single thread, so the thread mask is the process mask. */
int pthread_sigmask(int how, const sigset_t *restrict set, sigset_t *restrict oset) {
    return sigprocmask(how, set, oset) == 0 ? 0 : errno;
}
