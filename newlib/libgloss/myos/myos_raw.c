#include "myos_syscalls.h"

long myos_syscall0(long nr) {
    long ret;
#if defined(__x86_64__)
    __asm__ volatile("syscall" : "=a"(ret) : "a"(nr) : "rcx", "r11", "memory");
#elif defined(__aarch64__)
    register long x8 __asm__("x8") = nr;
    __asm__ volatile("svc #0" : "=r"(ret) : "r"(x8) : "memory");
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
#else
#error unsupported arch
#endif
    return ret;
}
