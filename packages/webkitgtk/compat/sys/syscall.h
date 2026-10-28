/* myos: WebKit (built as for Linux) includes <sys/syscall.h> for Linux's
 * system call numbers. myos's are its own, and the patch takes WebKit's
 * raw system calls away (gettid is the libc's): nothing here. */
#ifndef MYOS_COMPAT_SYS_SYSCALL_H
#define MYOS_COMPAT_SYS_SYSCALL_H
#endif
