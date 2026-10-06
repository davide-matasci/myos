/*
 * Kernel stress / hardening smoke: throw hostile arguments at every native
 * syscall from userspace and confirm the kernel never faults. Each call runs
 * in a forked child (disposable: a bad call that faults becomes SIGSEGV in
 * the child, never the kernel), with a spin-watchdog that SIGKILLs a child
 * that blocks. Progress is written unbuffered, so if the machine dies the
 * last "nr=" line names the syscall that did it.
 *
 * This is a probe, not a boot test: it is not in the image's test list. Build
 * it with the other c-smokes and run it by hand (or wire a `t` line locally).
 */
#define _GNU_SOURCE 1
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

long myos_syscall6(long nr, long a0, long a1, long a2, long a3, long a4, long a5);

static void puts_raw(const char *s) { write(1, s, strlen(s)); }

static void put_num(long n) {
    char b[24];
    int i = sizeof b;
    unsigned long v = (unsigned long)n;
    b[--i] = '\n';
    if (v == 0) {
        b[--i] = '0';
    }
    while (v && i > 0) {
        b[--i] = '0' + (v % 10);
        v /= 10;
    }
    write(1, b + i, sizeof b - i);
}

/* A page we own, for "valid pointer" arguments. */
static unsigned char page[8192] __attribute__((aligned(4096)));
/* A page filled with 0xff: passed where a syscall reads a struct or path
 * from the pointer, so every field it decodes is hostile (usize::MAX, a
 * non-canonical handler, a never-terminated path). */
static unsigned char hpage[8192] __attribute__((aligned(4096)));

/* Hostile scalar values reused for every argument position. */
static const unsigned long VALUES[] = {
    0,
    1,
    (unsigned long)-1,
    0x7fffffffffffffffUL,
    0x8000000000000000UL,
    0xffffffff80000000UL,  /* kernel higher-half (x86 layouts) */
    0xffff800000000000UL,  /* HHDM-ish */
    0xdead000000000000UL,  /* non-canonical */
    0xfff,                 /* unaligned, near zero page */
    0x1000,
    (unsigned long)(uintptr_t)page,        /* real mapped page */
    (unsigned long)(uintptr_t)page + 1,    /* real but unaligned */
};
#define NVAL (sizeof VALUES / sizeof VALUES[0])

/* Globally destructive calls: skip even in a child, since they change state
 * the whole machine (or this test's own init/getty) shares. */
static int denied(long nr) {
    switch (nr) {
    /* Globally destructive: would change state the whole machine shares. */
    case 6:   /* fork   */
    case 27:  /* mount  */
    case 34:  /* kill   */
    case 58:  /* insmod */
    case 59:  /* rmmod  */
    case 64:  /* umount */
    case 65:  /* settimeofday */
    case 69:  /* policy_load  */
    /* Blocking: they wait indefinitely, which the watchdog would have to
     * kill every time. We hunt arithmetic faults, not blocking behaviour. */
    case 7:   /* wait        */
    case 38:  /* poll        */
    case 46:  /* waitpid     */
    case 48:  /* sigsuspend  */
    case 49:  /* sigwait     */
    case 52:  /* nanosleep   */
    case 55:  /* wait_addr   */
    case 85:  /* flock       */
        return 1;
    default:
        return 0;
    }
}

/* Run one call in a child; SIGKILL it if it blocks. Returns nothing: the
 * point is whether the *kernel* survives, which the caller sees by living. */
static void try_call(long nr, long a0, long a1, long a2, long a3, long a4, long a5) {
    pid_t pid = fork();
    if (pid < 0) {
        return;
    }
    if (pid == 0) {
        myos_syscall6(nr, a0, a1, a2, a3, a4, a5);
        _exit(0);
    }
    int st;
    for (long i = 0; i < 200000; i++) {
        if (waitpid(pid, &st, WNOHANG) == pid) {
            return;
        }
    }
    kill(pid, SIGKILL);
    waitpid(pid, &st, 0);
}

int main(void) {
    memset(page, 0, sizeof page);
    memset(hpage, 0xff, sizeof hpage);
    puts_raw("crash_smoke: start\n");
    /* Real objects so the fd-taking syscalls (lseek, pread/pwrite, ftruncate,
     * flock, lockctl, fdflags, dup2) reach their offset/size arithmetic with a
     * valid fd. Each call runs in a child, so a child closing/truncating these
     * does not disturb the parent's copies. */
    int ffd = open("/tmp/crash_smoke.f", O_RDWR | O_CREAT | O_TRUNC, 0644);
    if (ffd >= 0) {
        write(ffd, "0123456789", 10);
    }
    int pfd[2];
    if (pipe(pfd) != 0) {
        pfd[0] = pfd[1] = -1;
    }
    /* A real directory fd, so the *at syscalls reach their dir-capability
     * path resolution with a valid base instead of bailing on a bad fd. */
    int dfd = open("/tmp", O_RDONLY | O_DIRECTORY);
    long fds[] = {ffd, pfd[0], pfd[1], dfd, 0, 1, 2};
    const int NFD = (int)(sizeof fds / sizeof fds[0]);

    for (long nr = 0; nr <= 90; nr++) {
        if (denied(nr)) {
            continue;
        }
        puts_raw("nr=");
        put_num(nr);
        /* All six args set to each hostile value. */
        for (unsigned v = 0; v < NVAL; v++) {
            unsigned long x = VALUES[v];
            try_call(nr, x, x, x, x, x, x);
        }
        /* A real buffer with a hostile length in the common (ptr,len) slots. */
        unsigned long p = (unsigned long)(uintptr_t)page;
        for (unsigned v = 0; v < NVAL; v++) {
            unsigned long len = VALUES[v];
            try_call(nr, p, len, p, len, p, len);
            try_call(nr, 0, p, len, p, len, p);
            try_call(nr, p, p, p, len, len, len);
        }
        /* A valid fd in arg0, hostile offsets/sizes/pointers after it. */
        for (int f = 0; f < NFD; f++) {
            for (unsigned v = 0; v < NVAL; v++) {
                unsigned long x = VALUES[v];
                try_call(nr, fds[f], x, x, x, x, x);
                try_call(nr, fds[f], p, x, x, x, x);
            }
        }
    }
    /* Struct/path-pointer pass: hand each syscall a pointer to an all-0xff
     * page, so the ones that decode a struct (mmap args, sigaction, thread
     * spawn params, poll sets) or a path see hostile contents rather than
     * the zeroed "valid pointer" the sweep above used. */
    unsigned long h = (unsigned long)(uintptr_t)hpage;
    for (long nr = 0; nr <= 90; nr++) {
        if (denied(nr)) {
            continue;
        }
        puts_raw("hnr=");
        put_num(nr);
        try_call(nr, h, h, h, h, h, h);
        try_call(nr, 0, h, h, h, h, h);
        for (int f = 0; f < NFD; f++) {
            try_call(nr, fds[f], h, h, h, h, h);
        }
    }
    puts_raw("crash_smoke: SURVIVED\n");
    return 0;
}
