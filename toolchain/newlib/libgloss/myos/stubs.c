/* myos libgloss: POSIX hooks with no kernel backing yet (errno = ENOSYS). */

#include <_ansi.h>
#include <errno.h>
#include <reent.h>
#include <stdint.h>
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


int _link(const char *oldpath, const char *newpath) {
    (void)oldpath;
    (void)newpath;
    errno = EROFS;
    return -1;
}

int _kill(int pid, int sig) {
    (void)pid;
    (void)sig;
    errno = ENOSYS;
    return -1;
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
    long ret = myos_syscall1(MYOS_SYS_WAIT, (long)(uintptr_t)status);
    if (ret == (long)MYOS_SYSERR) {
        errno = ECHILD;
        return -1;
    }
    return (int)ret;
}

/*
 * Pack argv/envp into the myos SYS_EXEC layout:
 *   [argc, (ptr,len)…, envc, (ptr,len)…]
 * Strings are copied onto the stack so the kernel copy-in sees writable user
 * memory (AArch64 ET_EXEC rodata VAs are rejected).
 */
#define MYOS_MAX_ARGC 16
#define MYOS_MAX_ARG_LEN 128
#define MYOS_MAX_ENVC 32
#define MYOS_MAX_ENV_LEN 128
#define MYOS_MAX_PATH 64

int _execve(const char *path, char *const argv[], char *const envp[]) {
    char path_buf[MYOS_MAX_PATH];
    char arg_store[MYOS_MAX_ARGC][MYOS_MAX_ARG_LEN];
    char env_store[MYOS_MAX_ENVC][MYOS_MAX_ENV_LEN];
    unsigned long pack[1 + MYOS_MAX_ARGC * 2 + 1 + MYOS_MAX_ENVC * 2];
    size_t path_len;
    int argc = 0;
    int envc = 0;
    int i;

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

    if (argv != NULL) {
        for (; argv[argc] != NULL; argc++) {
            size_t n;
            if (argc >= MYOS_MAX_ARGC) {
                errno = E2BIG;
                return -1;
            }
            n = strlen(argv[argc]);
            if (n >= MYOS_MAX_ARG_LEN) {
                errno = E2BIG;
                return -1;
            }
            memcpy(arg_store[argc], argv[argc], n);
            arg_store[argc][n] = '\0';
        }
    }
    if (envp != NULL) {
        for (; envp[envc] != NULL; envc++) {
            size_t n;
            if (envc >= MYOS_MAX_ENVC) {
                errno = E2BIG;
                return -1;
            }
            n = strlen(envp[envc]);
            if (n >= MYOS_MAX_ENV_LEN) {
                errno = E2BIG;
                return -1;
            }
            memcpy(env_store[envc], envp[envc], n);
            env_store[envc][n] = '\0';
        }
    }

    pack[0] = (unsigned long)argc;
    for (i = 0; i < argc; i++) {
        pack[1 + i * 2] = (unsigned long)(uintptr_t)arg_store[i];
        pack[2 + i * 2] = (unsigned long)strlen(arg_store[i]);
    }
    pack[1 + argc * 2] = (unsigned long)envc;
    for (i = 0; i < envc; i++) {
        pack[1 + argc * 2 + 1 + i * 2] = (unsigned long)(uintptr_t)env_store[i];
        pack[1 + argc * 2 + 2 + i * 2] = (unsigned long)strlen(env_store[i]);
    }

    long ret = myos_syscall3(
        MYOS_SYS_EXEC,
        (long)(uintptr_t)path_buf,
        (long)path_len,
        (long)(uintptr_t)pack);
    if (ret == (long)MYOS_SYSERR) {
        errno = ENOENT;
        return -1;
    }
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
    tv->tv_sec = 0;
    tv->tv_usec = 0;
    return 0;
}

int _chown(const char *path, uid_t owner, gid_t group) {
    (void)path;
    (void)owner;
    (void)group;
    errno = EROFS;
    return -1;
}

int _mknod(const char *path, mode_t mode, dev_t dev) {
    (void)path;
    (void)mode;
    (void)dev;
    errno = EROFS;
    return -1;
}

int _mkfifo(const char *path, mode_t mode) {
    (void)path;
    (void)mode;
    errno = EROFS;
    return -1;
}
