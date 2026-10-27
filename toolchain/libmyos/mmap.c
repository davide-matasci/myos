/* mmap / munmap / mprotect / msync / madvise. */
#include <errno.h>
#include <sys/types.h>
#include <stdint.h>
#include <sys/mman.h>
#include <unistd.h>

#include "myos_syscalls.h"
#include "libc.h"

void *__myos_mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset) {
    struct myos_mmap_args args;
    long ret;

    args.addr = (unsigned long)(uintptr_t)addr;
    args.length = (unsigned long)length;
    args.prot = (unsigned long)prot;
    args.flags = (unsigned long)flags;
    args.fd = (long)fd;
    args.offset = (unsigned long)offset;
    ret = myos_syscall3(MYOS_SYS_MMAP, (long)(uintptr_t)&args, 0, 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = ENOMEM;
        return MAP_FAILED;
    }
    return (void *)(uintptr_t)ret;
}

int __myos_munmap(void *addr, size_t length) {
    long ret = myos_syscall3(
        MYOS_SYS_MUNMAP, (long)(uintptr_t)addr, (long)length, 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = EINVAL;
        return -1;
    }
    return 0;
}

int __myos_mprotect(void *addr, size_t length, int prot) {
    long ret = myos_syscall3(
        MYOS_SYS_MPROTECT, (long)(uintptr_t)addr, (long)length, (long)prot);
    if (ret == (long)MYOS_SYSERR) {
        errno = EINVAL;
        return -1;
    }
    return 0;
}

int __myos_msync(void *addr, size_t length, int flags) {
    long ret;
    if (flags & ~(MS_ASYNC | MS_INVALIDATE | MS_SYNC) || ((flags & MS_ASYNC) && (flags & MS_SYNC))) {
        errno = EINVAL;
        return -1;
    }
    ret = myos_syscall3(MYOS_SYS_MSYNC, (long)(uintptr_t)addr, (long)length, (long)flags);
    if (ret == (long)MYOS_SYSERR) {
        errno = ENOMEM;
        return -1;
    }
    return 0;
}

int __myos_madvise(void *addr, size_t length, int advice) {
    long ret = myos_syscall3(
        MYOS_SYS_MADVISE, (long)(uintptr_t)addr, (long)length, (long)advice);
    if (ret == (long)MYOS_SYSERR) {
        errno = EINVAL;
        return -1;
    }
    return 0;
}

void __clear_cache(void *start, void *end) {
#if defined(__aarch64__)
    unsigned long s = (unsigned long)start & ~63ul;
    unsigned long e = ((unsigned long)end + 63ul) & ~63ul;
    unsigned long p;
    for (p = s; p < e; p += 64) {
        __asm__ volatile("dc cvau, %0" :: "r"(p) : "memory");
    }
    __asm__ volatile("dsb ish" ::: "memory");
    for (p = s; p < e; p += 64) {
        __asm__ volatile("ic ivau, %0" :: "r"(p) : "memory");
    }
    __asm__ volatile("dsb ish; isb" ::: "memory");
#elif defined(__riscv)
    (void)start;
    (void)end;
    __asm__ volatile("fence.i" ::: "memory");
#else
    (void)start;
    (void)end;
#endif
}

MYOS_ALIAS(mmap)
MYOS_ALIAS(munmap)
MYOS_ALIAS(mprotect)
MYOS_ALIAS(msync)
MYOS_PUBLIC(madvise)
