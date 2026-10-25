/*
 * Faults, and what stops them.
 *
 * A slave write to a pty whose master is open but never read fills the
 * output ring and blocks; the blocked write must be interruptible by a
 * signal, not a 100%-CPU unkillable kernel spin (kernel/src/pty.rs
 * `slave_write` once read the wait sequence before its own `notify`, so it
 * busy-returned from `block_until` forever and never checked for signals —
 * not even SIGKILL could stop it). The child fills the ring and blocks
 * writing; the parent SIGKILLs it and reaps it. If the write were
 * unkillable the reap would hang and the boot would time out.
 *
 * A CPU fault raises its signal (kernel/src/signal.rs `on_fault`): an
 * undefined instruction SIGILL, a breakpoint SIGTRAP, a load from a
 * PROT_NONE page or from an unmapped one SIGSEGV (SEGV_ACCERR, SEGV_MAPERR,
 * the address in si_addr), an integer divide by zero SIGFPE on x86 (the
 * other two do not trap), and x86's single step SIGTRAP, its handler
 * entered with TF clear. Those handlers leave with siglongjmp, which puts
 * the mask back. A SIGSEGV handler that makes the page writable returns
 * instead, and the store runs again with the registers it had: the ones a
 * syscall's return does not keep (x86's rcx, r11 and flags, aarch64's
 * NZCV, the result and number registers) are checked after it. A fault the
 * program does not catch still ends it, but a test cannot fault that way:
 * the host takes the kernel's `user fault` line for a failure.
 *
 * Prints one `[ OK ] fault` / `[ FAIL ] fault ...` line.
 */
#define _GNU_SOURCE 1
#include <errno.h>
#include <pty.h>
#include <setjmp.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

static const char *pty_writer(void) {
    int m, s;
    char name[64];
    if (openpty(&m, &s, name, NULL, NULL) != 0) {
        return "openpty";
    }
    pid_t p = fork();
    if (p == 0) {
        // Master stays open (in the parent and here) but nobody reads it, so
        // the ring fills after OUT_CAP bytes and this write blocks.
        static char buf[65536];
        memset(buf, 'x', sizeof buf);
        while (write(s, buf, sizeof buf) > 0) {
        }
        _exit(0);
    }
    // Give the child time to fill the ring and block in the kernel.
    struct timespec ts = {0, 400 * 1000 * 1000};
    nanosleep(&ts, NULL);
    // A blocked slave writer must be killable.
    kill(p, SIGKILL);
    int st = 0;
    (void)waitpid(p, &st, 0);
    close(m);
    close(s);
    if (!WIFSIGNALED(st) || WTERMSIG(st) != SIGKILL) {
        return "blocked pty writer not killed by SIGKILL";
    }
    return NULL;
}

static sigjmp_buf env;
static volatile int got_sig, got_code;
static void *volatile got_addr;
/* What the SIGSEGV cases touch, and the page a returning handler fixes. */
static volatile char *target;
static char *page;
static size_t pagesz;

static void on_fault_leave(int sig, siginfo_t *si, void *uc) {
    (void)uc;
    got_sig = sig;
    got_code = si->si_code;
    got_addr = si->si_addr;
    siglongjmp(env, 1);
}

static void on_fault_fix(int sig, siginfo_t *si, void *uc) {
    (void)uc;
    got_sig = sig;
    got_code = si->si_code;
    got_addr = si->si_addr;
    mprotect(page, pagesz, PROT_READ | PROT_WRITE);
}

static void catch_with(int sig, void (*fn)(int, siginfo_t *, void *)) {
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = fn;
    sa.sa_flags = SA_SIGINFO;
    sigemptyset(&sa.sa_mask);
    sigaction(sig, &sa, NULL);
}

static void undefined_insn(void) {
#if defined(__x86_64__)
    __asm__ volatile("ud2");
#elif defined(__aarch64__)
    __asm__ volatile("udf #0");
#else
    __asm__ volatile("unimp");
#endif
}

static void breakpoint(void) {
#if defined(__x86_64__)
    __asm__ volatile("int3");
#elif defined(__aarch64__)
    __asm__ volatile("brk #0");
#else
    __asm__ volatile("ebreak");
#endif
}

static void load_target(void) {
    (void)*target;
}

#if defined(__x86_64__)
/* TF set: the instruction after popfq traps (SIGTRAP, TRAP_TRACE). The
 * handler starts with TF clear (else it would trap again with SIGTRAP
 * blocked and the task end) and leaves with siglongjmp, TF still clear. */
static void single_step(void) {
    __asm__ volatile("pushfq\n\torq $0x100, (%%rsp)\n\tpopfq\n\tnop\n\tnop" ::: "memory", "cc");
}

static void divide_by_zero(void) {
    __asm__ volatile("xor %%ecx, %%ecx\n\tmov $1, %%eax\n\tcltd\n\tidivl %%ecx" ::: "eax", "ecx", "edx", "cc");
}
#endif

/* `run` raises `sig` with `code` (and `addr`, unless NULL), which a handler
 * leaves with siglongjmp: the error, or NULL. */
static const char *raises(void (*run)(void), int sig, int code, void *addr) {
    catch_with(sig, on_fault_leave);
    got_sig = 0;
    if (sigsetjmp(env, 1) == 0) {
        run();
        return "no signal";
    }
    signal(sig, SIG_DFL);
    if (got_sig != sig) {
        return "another signal";
    }
    if (got_code != code) {
        return "wrong si_code";
    }
    if (addr != NULL ? got_addr != addr : got_addr == NULL) {
        return "wrong si_addr";
    }
    // siglongjmp put back the mask from before the handler blocked `sig`.
    sigset_t now;
    sigprocmask(SIG_BLOCK, NULL, &now);
    if (sigismember(&now, sig)) {
        return "still blocked after siglongjmp";
    }
    return NULL;
}

/* A store to the PROT_NONE `p` whose handler makes it writable and returns:
 * the store runs again, and the registers the kernel's syscall return does
 * not keep are as they were. */
static int store_resumes(volatile char *p) {
#if defined(__x86_64__)
    register unsigned long rax __asm__("rax") = 0x1111111111111111UL;
    register unsigned long rcx __asm__("rcx") = 0x2222222222222222UL;
    register unsigned long r11 __asm__("r11") = 0x3333333333333333UL;
    unsigned char cf;
    __asm__ volatile("stc\n\tmovb $1, (%[p])\n\tsetc %[cf]"
                     : "+r"(rax), "+r"(rcx), "+r"(r11), [cf] "=r"(cf)
                     : [p] "r"(p)
                     : "memory");
    return rax == 0x1111111111111111UL && rcx == 0x2222222222222222UL && r11 == 0x3333333333333333UL && cf == 1 &&
           p[0] == 1;
#elif defined(__aarch64__)
    register unsigned long x0 __asm__("x0") = 0x1111111111111111UL;
    register unsigned long x8 __asm__("x8") = 0x2222222222222222UL;
    unsigned long nzcv;
    // cmp xzr, xzr: Z and C set.
    __asm__ volatile("cmp xzr, xzr\n\tstrb %w[one], [%[p]]\n\tmrs %[f], nzcv"
                     : "+r"(x0), "+r"(x8), [f] "=r"(nzcv)
                     : [p] "r"(p), [one] "r"(1)
                     : "memory", "cc");
    return x0 == 0x1111111111111111UL && x8 == 0x2222222222222222UL && nzcv >> 28 == 0x6 && p[0] == 1;
#else
    register unsigned long a0 __asm__("a0") = 0x1111111111111111UL;
    register unsigned long a7 __asm__("a7") = 0x2222222222222222UL;
    __asm__ volatile("sb %[one], 0(%[p])" : "+r"(a0), "+r"(a7) : [p] "r"(p), [one] "r"(1) : "memory");
    return a0 == 0x1111111111111111UL && a7 == 0x2222222222222222UL && p[0] == 1;
#endif
}

static const char *fault_signals(void) {
    const char *e;
#if defined(__x86_64__)
    int ill = ILL_ILLOPN;
#else
    int ill = ILL_ILLOPC;
#endif
    if ((e = raises(undefined_insn, SIGILL, ill, NULL)) != NULL) {
        return e;
    }
    if ((e = raises(breakpoint, SIGTRAP, TRAP_BRKPT, NULL)) != NULL) {
        return e;
    }
#if defined(__x86_64__)
    if ((e = raises(divide_by_zero, SIGFPE, FPE_INTDIV, NULL)) != NULL) {
        return e;
    }
    if ((e = raises(single_step, SIGTRAP, TRAP_TRACE, NULL)) != NULL) {
        return e;
    }
    unsigned long flags;
    __asm__ volatile("pushfq\n\tpopq %0" : "=r"(flags));
    if (flags & 0x100) {
        return "TF still set after the single-step handler";
    }
#endif
    pagesz = (size_t)sysconf(_SC_PAGESIZE);
    page = mmap(NULL, pagesz, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (page == MAP_FAILED) {
        return "mmap";
    }
    target = page + 8;
    if ((e = raises(load_target, SIGSEGV, SEGV_ACCERR, page + 8)) != NULL) {
        return e;
    }
    char *gone = mmap(NULL, pagesz, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (gone == MAP_FAILED || munmap(gone, pagesz) != 0) {
        return "mmap";
    }
    target = gone;
    if ((e = raises(load_target, SIGSEGV, SEGV_MAPERR, gone)) != NULL) {
        return e;
    }
    catch_with(SIGSEGV, on_fault_fix);
    got_sig = 0;
    int kept = store_resumes(page + 16);
    signal(SIGSEGV, SIG_DFL);
    if (got_sig != SIGSEGV || got_addr != page + 16) {
        return "the returning handler did not run";
    }
    if (!kept) {
        return "the store did not resume with its registers";
    }
    munmap(page, pagesz);
    return NULL;
}

int main(void) {
    const char *e = pty_writer();
    if (e == NULL) {
        e = fault_signals();
    }
    if (e != NULL) {
        printf("[ FAIL ] fault: %s (%s)\n", e, strerror(errno));
        return 1;
    }
    printf("[ OK ] fault\n");
    return 0;
}
