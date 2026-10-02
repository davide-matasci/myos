/* myos libgloss: POSIX hooks with no kernel backing yet (errno = ENOSYS). */

#include <_ansi.h>
#include <errno.h>
#include <reent.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/times.h>
#include <unistd.h>

#include "myos_syscalls.h"

int _lseek(int fd, off_t pos, int whence) {
    long ret = myos_syscall3(MYOS_SYS_LSEEK, fd, (long)pos, whence);
    if (ret == (long)MYOS_SYSERR) {
        errno = ESPIPE;
        return -1;
    }
    return (int)ret;
}

int _unlink(const char *path) {
    if (path == NULL) {
        errno = ENOENT;
        return -1;
    }
    long ret = myos_syscall3(
        MYOS_SYS_UNLINK, (long)(uintptr_t)path, (long)strlen(path), 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = ENOENT;
        return -1;
    }
    return 0;
}

int _rename(const char *oldpath, const char *newpath) {
    size_t old_len;
    size_t new_len;
    long packed;
    long ret;

    if (oldpath == NULL || newpath == NULL) {
        errno = ENOENT;
        return -1;
    }
    old_len = strlen(oldpath);
    new_len = strlen(newpath);
    if (old_len == 0 || new_len == 0 || old_len > 0xffff || new_len > 0xffff) {
        errno = ENAMETOOLONG;
        return -1;
    }
    packed = (long)((old_len << 16) | new_len);
    ret = myos_syscall3(
        MYOS_SYS_RENAME,
        (long)(uintptr_t)oldpath,
        (long)(uintptr_t)newpath,
        packed);
    if (ret == (long)MYOS_SYSERR) {
        errno = ENOENT;
        return -1;
    }
    return 0;
}


/* Hardlink not implemented. Newlib rename() must use HAVE_RENAME → _rename;
 * without that it falls back to link+unlink and surfaces EROFS on git init. */
int _link(const char *oldpath, const char *newpath) {
    (void)oldpath;
    (void)newpath;
    errno = EROFS;
    return -1;
}

int _kill(int pid, int sig) {
    /* Signal numbers must match newlib <signal.h> / kernel signal.rs. */
    if (sig <= 0 || sig > 31) {
        errno = EINVAL;
        return -1;
    }
    long ret = myos_syscall3(MYOS_SYS_KILL, (long)pid, (long)sig, 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = ESRCH;
        return -1;
    }
    return 0;
}

int _fork(void) {
    long ret = myos_syscall0(MYOS_SYS_FORK);
    if (ret == (long)MYOS_SYSERR) {
        errno = EAGAIN;
        return -1;
    }
    return (int)ret;
}

int _wait(int *status) {
    int st = 0;
    long ret = myos_syscall3(MYOS_SYS_WAITPID, (long)(uintptr_t)&st, 0, 0);
    if (ret == (long)MYOS_EINTR) {
        errno = EINTR;
        return -1;
    }
    if (ret == (long)MYOS_SYSERR) {
        errno = ECHILD;
        return -1;
    }
    if (status != NULL) {
        *status = st;
    }
    return (int)ret;
}

/*
 * Pack argv/envp into the myos SYS_EXEC layout:
 *   [argc, (ptr,len)…, envc, (ptr,len)…]
 * Strings are copied into one heap block so the kernel copy-in sees writable
 * user memory (AArch64 ET_EXEC rodata VAs are rejected). The limits match the
 * kernel's (kernel/src/user/mod.rs): MAX_ARGC, MAX_ENVC, and MAX_EXEC_STRINGS
 * bytes for all strings with their NULs.
 */
#define MYOS_MAX_ARGC 1024
#define MYOS_MAX_ENVC 1024
#define MYOS_MAX_EXEC_STRINGS (128 * 1024)
/* Matches the kernel exec path limit (kernel/src/user/mod.rs MAX_PATH). */
#define MYOS_MAX_PATH 256

int _execve(const char *path, char *const argv[], char *const envp[]) {
    char path_buf[MYOS_MAX_PATH];
    size_t path_len;
    size_t bytes = 0;
    size_t argc = 0;
    size_t envc = 0;
    size_t i;
    unsigned long *pack;
    char *store;
    long ret;

    if (path == NULL) {
        errno = EFAULT;
        return -1;
    }
    path_len = strlen(path);
    if (path_len == 0 || path_len >= MYOS_MAX_PATH) {
        errno = ENAMETOOLONG;
        return -1;
    }
    memcpy(path_buf, path, path_len);
    path_buf[path_len] = '\0';

    for (; argv != NULL && argv[argc] != NULL; argc++) {
        bytes += strlen(argv[argc]) + 1;
    }
    for (; envp != NULL && envp[envc] != NULL; envc++) {
        bytes += strlen(envp[envc]) + 1;
    }
    if (argc > MYOS_MAX_ARGC || envc > MYOS_MAX_ENVC || bytes > MYOS_MAX_EXEC_STRINGS) {
        errno = E2BIG;
        return -1;
    }
    pack = malloc((2 + 2 * (argc + envc)) * sizeof(unsigned long) + bytes);
    if (pack == NULL) {
        errno = ENOMEM;
        return -1;
    }
    store = (char *)(pack + 2 + 2 * (argc + envc));

    pack[0] = (unsigned long)argc;
    for (i = 0; i < argc; i++) {
        size_t n = strlen(argv[i]);
        memcpy(store, argv[i], n + 1);
        pack[1 + i * 2] = (unsigned long)(uintptr_t)store;
        pack[2 + i * 2] = (unsigned long)n;
        store += n + 1;
    }
    pack[1 + argc * 2] = (unsigned long)envc;
    for (i = 0; i < envc; i++) {
        size_t n = strlen(envp[i]);
        memcpy(store, envp[i], n + 1);
        pack[1 + argc * 2 + 1 + i * 2] = (unsigned long)(uintptr_t)store;
        pack[1 + argc * 2 + 2 + i * 2] = (unsigned long)n;
        store += n + 1;
    }

    ret = myos_syscall3(
        MYOS_SYS_EXEC,
        (long)(uintptr_t)path_buf,
        (long)path_len,
        (long)(uintptr_t)pack);
    /* Only reached on failure. */
    (void)ret;
    free(pack);
    errno = ENOENT;
    return -1;
}

clock_t _times(struct tms *buf) {
    (void)buf;
    errno = ENOSYS;
    return (clock_t)-1;
}

int _gettimeofday(struct timeval *tv, void *tz) {
    (void)tz;
    if (tv == NULL) {
        errno = EINVAL;
        return -1;
    }
    long ret = myos_syscall3(
        MYOS_SYS_GETTIMEOFDAY, (long)(uintptr_t)tv, 0, 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = EIO;
        return -1;
    }
    return 0;
}

int _chown(const char *path, uid_t owner, gid_t group) {
    (void)path;
    (void)owner;
    (void)group;
    errno = EROFS;
    return -1;
}

int mknod(const char *path, mode_t mode, dev_t dev); /* posix_stubs.c */

int _mknod(const char *path, mode_t mode, dev_t dev) {
    return mknod(path, mode, dev);
}

int _mkfifo(const char *path, mode_t mode) {
    return mkfifo(path, mode);
}
