/*
 * linux [--root DIR] PROGRAM [ARG...]
 *
 * Run a Linux (musl) binary through the kernel's optional Linux syscall
 * compatibility layer: ask the kernel to start the next exec with the Linux
 * personality, then exec. Needs a kernel built with the linux-compat
 * feature; see docs/linux-compat.md.
 *
 * --root DIR runs it chrooted into DIR (a Linux root such as get-void's,
 * where /lib/ld-musl-*.so.1 and the shared objects live), with PROGRAM
 * looked up in a Linux PATH. The Linux layer still shows such a process the
 * real /dev and /proc.
 *
 * Uses fputs, not fprintf: newlib's printf needs extra soft-float helpers
 * on aarch64/riscv64.
 */
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* kernel/src/user/syscall.rs SYS_LINUX_NEXT_EXEC */
#define MYOS_SYS_LINUX_NEXT_EXEC 51

static long linux_next_exec(void) {
#if defined(__x86_64__)
    long ret;
    __asm__ volatile("syscall"
                     : "=a"(ret)
                     : "a"((long)MYOS_SYS_LINUX_NEXT_EXEC)
                     : "rcx", "r11", "memory");
    return ret;
#elif defined(__aarch64__)
    register long x8 __asm__("x8") = MYOS_SYS_LINUX_NEXT_EXEC;
    register long x0 __asm__("x0");
    __asm__ volatile("svc #0" : "=r"(x0) : "r"(x8) : "memory");
    return x0;
#elif defined(__riscv)
    register long a7 __asm__("a7") = MYOS_SYS_LINUX_NEXT_EXEC;
    register long a0 __asm__("a0");
    __asm__ volatile("ecall" : "=r"(a0) : "r"(a7) : "memory");
    return a0;
#else
#error "unsupported arch"
#endif
}

static int fail(const char *what, const char *arg) {
    int e = errno;
    fputs("linux: ", stderr);
    fputs(what, stderr);
    fputs(arg, stderr);
    fputs(": ", stderr);
    fputs(strerror(e), stderr);
    fputs("\n", stderr);
    return 127;
}

int main(int argc, char **argv) {
    const char *root = NULL;
    if (argc >= 3 && strcmp(argv[1], "--root") == 0) {
        root = argv[2];
        argv += 2;
        argc -= 2;
    }
    if (argc < 2) {
        fputs("usage: linux [--root DIR] PROGRAM [ARG...]\n", stderr);
        return 2;
    }
    if (root != NULL) {
        if (chroot(root) != 0) {
            return fail("chroot ", root);
        }
        if (chdir("/") != 0) {
            return fail("chdir ", "/");
        }
        setenv("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin", 1);
    }
    if (linux_next_exec() != 0) {
        fputs("linux: this kernel has no Linux compatibility layer\n", stderr);
        return 126;
    }
    execvp(argv[1], argv + 1);
    return fail("", argv[1]);
}
