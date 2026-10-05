#include "myos_syscalls.h"

long myos_syscall0(long nr) {
    long ret;
#if defined(__x86_64__)
    __asm__ volatile("syscall" : "=a"(ret) : "a"(nr) : "rcx", "r11", "memory");
#elif defined(__aarch64__)
    register long x8 __asm__("x8") = nr;
    __asm__ volatile("svc #0" : "=r"(ret) : "r"(x8) : "memory");
#elif defined(__riscv) && __riscv_xlen == 64
    register long a7 __asm__("a7") = nr;
    __asm__ volatile("ecall" : "=r"(ret) : "r"(a7) : "memory");
#else
#error unsupported arch
#endif
    return ret;
}

long myos_syscall1(long nr, long a0) {
    long ret;
#if defined(__x86_64__)
    __asm__ volatile("syscall" : "=a"(ret) : "a"(nr), "D"(a0) : "rcx", "r11", "memory");
#elif defined(__aarch64__)
    register long x8 __asm__("x8") = nr;
    register long x0 __asm__("x0") = a0;
    __asm__ volatile("svc #0" : "=r"(x0) : "r"(x8), "r"(x0) : "memory");
    ret = x0;
#elif defined(__riscv) && __riscv_xlen == 64
    register long a7 __asm__("a7") = nr;
    register long a0reg __asm__("a0") = a0;
    __asm__ volatile("ecall" : "=r"(a0reg) : "r"(a7), "r"(a0reg) : "memory");
    ret = a0reg;
#else
#error unsupported arch
#endif
    return ret;
}

long myos_syscall2(long nr, long a0, long a1) {
    return myos_syscall3(nr, a0, a1, 0);
}


long myos_syscall3(long nr, long a0, long a1, long a2) {
    long ret;
#if defined(__x86_64__)
    __asm__ volatile("syscall"
                     : "=a"(ret)
                     : "a"(nr), "D"(a0), "S"(a1), "d"(a2)
                     : "rcx", "r11", "memory");
#elif defined(__aarch64__)
    register long x8 __asm__("x8") = nr;
    register long x0 __asm__("x0") = a0;
    register long x1 __asm__("x1") = a1;
    register long x2 __asm__("x2") = a2;
    __asm__ volatile("svc #0"
                     : "=r"(x0)
                     : "r"(x8), "r"(x0), "r"(x1), "r"(x2)
                     : "memory");
    ret = x0;
#elif defined(__riscv) && __riscv_xlen == 64
    register long a7 __asm__("a7") = nr;
    register long a0reg __asm__("a0") = a0;
    register long a1reg __asm__("a1") = a1;
    register long a2reg __asm__("a2") = a2;
    __asm__ volatile("ecall"
                     : "=r"(a0reg)
                     : "r"(a7), "r"(a0reg), "r"(a1reg), "r"(a2reg)
                     : "memory");
    ret = a0reg;
#else
#error unsupported arch
#endif
    return ret;
}

/* Six arguments: x86-64 passes the fourth to sixth in r10, r8 and r9
 * (rcx and r11 are the syscall instruction's). */
long myos_syscall6(long nr, long a0, long a1, long a2, long a3, long a4, long a5) {
    long ret;
#if defined(__x86_64__)
    register long r10 __asm__("r10") = a3;
    register long r8 __asm__("r8") = a4;
    register long r9 __asm__("r9") = a5;
    __asm__ volatile("syscall"
                     : "=a"(ret)
                     : "a"(nr), "D"(a0), "S"(a1), "d"(a2), "r"(r10), "r"(r8), "r"(r9)
                     : "rcx", "r11", "memory");
#elif defined(__aarch64__)
    register long x8 __asm__("x8") = nr;
    register long x0 __asm__("x0") = a0;
    register long x1 __asm__("x1") = a1;
    register long x2 __asm__("x2") = a2;
    register long x3 __asm__("x3") = a3;
    register long x4 __asm__("x4") = a4;
    register long x5 __asm__("x5") = a5;
    __asm__ volatile("svc #0"
                     : "=r"(x0)
                     : "r"(x8), "r"(x0), "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5)
                     : "memory");
    ret = x0;
#elif defined(__riscv) && __riscv_xlen == 64
    register long a7 __asm__("a7") = nr;
    register long a0reg __asm__("a0") = a0;
    register long a1reg __asm__("a1") = a1;
    register long a2reg __asm__("a2") = a2;
    register long a3reg __asm__("a3") = a3;
    register long a4reg __asm__("a4") = a4;
    register long a5reg __asm__("a5") = a5;
    __asm__ volatile("ecall"
                     : "=r"(a0reg)
                     : "r"(a7), "r"(a0reg), "r"(a1reg), "r"(a2reg), "r"(a3reg), "r"(a4reg), "r"(a5reg)
                     : "memory");
    ret = a0reg;
#else
#error unsupported arch
#endif
    return ret;
}
