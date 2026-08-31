#include <myos/syscall.h>

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
