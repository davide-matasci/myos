/*
 * linux PROGRAM [ARG...]
 *
 * Run a Linux (static-PIE musl, x86_64) binary through the kernel's optional
 * Linux syscall compatibility layer: ask the kernel to start the next exec
 * with the Linux personality, then exec. Needs a kernel built with the
 * linux-compat feature; see docs/linux-compat.md.
 */
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

/* kernel/src/user/syscall.rs SYS_LINUX_NEXT_EXEC */
#define MYOS_SYS_LINUX_NEXT_EXEC 51

static long linux_next_exec(void) {
    long ret;
    __asm__ volatile("syscall"
                     : "=a"(ret)
                     : "a"((long)MYOS_SYS_LINUX_NEXT_EXEC)
                     : "rcx", "r11", "memory");
    return ret;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: linux PROGRAM [ARG...]\n");
        return 2;
    }
    if (linux_next_exec() != 0) {
        fprintf(stderr, "linux: this kernel has no Linux compatibility layer\n");
        return 126;
    }
    execvp(argv[1], argv + 1);
    fprintf(stderr, "linux: %s: %s\n", argv[1], strerror(errno));
    return 127;
}
