/* myos libgloss: the path calls. The kernel's are all `*at` calls
 * (kernel/src/user/at.rs): a directory fd and a path relative to it, the
 * fd's own file for an empty path with AT_EMPTY_PATH. The plain calls are
 * the `*at` ones on AT_FDCWD (the cwd); fstat, futimens, fchdir and
 * fexecve are the `*at` ones on the fd itself.
 *
 * newlib's AT_* values (sys/_default_fcntl.h) and open flags are not the
 * kernel's: they are mapped here. The kernel reports one failure value for
 * most errors; errno is worked out from what is there. */

#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/myos_extra.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <unistd.h>

#include "myos_stat.h"
#include "myos_syscalls.h"

/* newlib declares it for _GNU_SOURCE only. */
#ifndef AT_EMPTY_PATH
#define AT_EMPTY_PATH 0x0010
#endif

/* The longest path the kernel takes (kernel/src/user/mod.rs MAX_PATH). */
#define MYOS_MAX_PATH 256

void myos_fd_nonblock_set(int fd, int on);

static long k_dirfd(int dirfd) {
    return dirfd == AT_FDCWD ? MYOS_AT_FDCWD : (long)dirfd;
}

static long k_flags(int flags) {
    long k = 0;
    if (flags & AT_SYMLINK_NOFOLLOW) {
        k |= MYOS_AT_SYMLINK_NOFOLLOW;
    }
    if (flags & AT_REMOVEDIR) {
        k |= MYOS_AT_REMOVEDIR;
    }
    if (flags & AT_EMPTY_PATH) {
        k |= MYOS_AT_EMPTY_PATH;
    }
    return k;
}

/* The length of `path` for the kernel: -1 with errno set when there is none
 * or it is too long. An empty one is the caller's to allow (AT_EMPTY_PATH). */
static long k_len(const char *path) {
    size_t n;
    if (path == NULL) {
        errno = EFAULT;
        return -1;
    }
    n = strlen(path);
    if (n > MYOS_MAX_PATH) {
        errno = ENAMETOOLONG;
        return -1;
    }
    return (long)n;
}

/* A path a call needs non-empty (no AT_EMPTY_PATH). */
static long k_len_named(const char *path) {
    long n = k_len(path);
    if (n == 0) {
        errno = ENOENT;
        return -1;
    }
    return n;
}

static int failed(long ret) {
    return ret == (long)MYOS_SYSERR;
}

/* ---- stat ---------------------------------------------------------------- */

static void fill_stat(struct stat *st, const struct myos_stat *src) {
    memset(st, 0, sizeof(*st));
    st->st_mode = src->st_mode;
    st->st_size = (off_t)src->st_size;
    st->st_ino = src->st_ino;
    st->st_nlink = src->st_nlink;
    st->st_dev = (dev_t)src->st_dev;
    st->st_uid = src->uid;
    st->st_gid = src->gid;
    st->st_blksize = 4096;
    st->st_blocks = (src->st_size + 511) / 512;
    st->st_atime = (time_t)src->atime;
    st->st_mtime = (time_t)src->mtime;
    /* No separate change time: the last modification stands in for it. */
    st->st_ctime = (time_t)src->mtime;
}

int fstatat(int dirfd, const char *path, struct stat *st, int flags) {
    struct myos_stat buf;
    long len = (flags & AT_EMPTY_PATH) ? k_len(path) : k_len_named(path);
    if (len < 0) {
        return -1;
    }
    if (st == NULL) {
        errno = EFAULT;
        return -1;
    }
    if (failed(myos_syscall6(MYOS_SYS_STATAT, k_dirfd(dirfd), (long)(uintptr_t)path, len,
                             k_flags(flags), (long)(uintptr_t)&buf, 0))) {
        errno = len == 0 ? EBADF : ENOENT;
        return -1;
    }
    fill_stat(st, &buf);
    return 0;
}

int _stat(const char *path, struct stat *st) {
    return fstatat(AT_FDCWD, path, st, 0);
}

int _lstat(const char *path, struct stat *st) {
    return fstatat(AT_FDCWD, path, st, AT_SYMLINK_NOFOLLOW);
}

int lstat(const char *path, struct stat *st) {
    return _lstat(path, st);
}

int _fstat(int fd, struct stat *st) {
    if (st == NULL) {
        errno = EFAULT;
        return -1;
    }
    /* A terminal reports its fd as its device (what ttyname compares). */
    if (myos_fd_is_tty(fd)) {
        memset(st, 0, sizeof(*st));
        st->st_mode = S_IFCHR | 0666;
        st->st_rdev = (dev_t)fd;
        st->st_nlink = 1;
        return 0;
    }
    return fstatat(fd, "", st, AT_EMPTY_PATH);
}

int faccessat(int dirfd, const char *path, int mode, int flags) {
    struct stat st;
    (void)mode;
    return fstatat(dirfd, path, &st, flags & AT_SYMLINK_NOFOLLOW);
}

int access(const char *path, int mode) {
    return faccessat(AT_FDCWD, path, mode, 0);
}

int _access(const char *path, int mode) {
    return access(path, mode);
}

/* ---- open ---------------------------------------------------------------- */

/*
 * The kernel's open flags are Linux-shaped (O_CREAT 0100, O_TRUNC 01000,
 * O_APPEND 02000). newlib <fcntl.h> is BSD-shaped (O_CREAT 0x0200,
 * O_TRUNC 0x0400, O_APPEND 0x0008). O_ACCMODE already matches.
 */
#define MYOS_K_O_CREAT 0x40
#define MYOS_K_O_TRUNC 0x200
#define MYOS_K_O_APPEND 0x400
#define MYOS_K_O_NONBLOCK 0x800
#define MYOS_K_O_EXCL 0x80
#define MYOS_K_O_CLOEXEC 0x80000
#define MYOS_K_O_DIRECTORY 0x10000
#define MYOS_K_O_NOFOLLOW 0x20000

static long k_oflags(int flags) {
    long k = (long)(flags & O_ACCMODE);
    if (flags & O_CREAT) {
        k |= MYOS_K_O_CREAT;
    }
    if (flags & O_TRUNC) {
        k |= MYOS_K_O_TRUNC;
    }
    if (flags & O_APPEND) {
        k |= MYOS_K_O_APPEND;
    }
    if (flags & O_NONBLOCK) {
        k |= MYOS_K_O_NONBLOCK; /* FIFO open: no wait for the peer */
    }
    if (flags & O_EXCL) {
        k |= MYOS_K_O_EXCL; /* with O_CREAT: only a new file */
    }
    if (flags & O_CLOEXEC) {
        k |= MYOS_K_O_CLOEXEC;
    }
    if (flags & O_NOFOLLOW) {
        k |= MYOS_K_O_NOFOLLOW; /* ELOOP for a symlink */
    }
    if (flags & O_DIRECTORY) {
        k |= MYOS_K_O_DIRECTORY; /* ENOTDIR for anything but a directory */
    }
    return k;
}

int openat(int dirfd, const char *path, int flags, ...) {
    struct stat st;
    long len = k_len_named(path);
    long ret;
    if (len < 0) {
        return -1;
    }
    ret = myos_syscall6(MYOS_SYS_OPENAT, k_dirfd(dirfd), (long)(uintptr_t)path, len, k_oflags(flags), 0, 0);
    if (ret == (long)MYOS_EEXIST) {
        errno = EEXIST; /* O_CREAT|O_EXCL: the name is taken (mkstemp tries another) */
        return -1;
    }
    if (ret == (long)MYOS_ELOOP || ret == (long)MYOS_ENOTDIR) {
        errno = ret == (long)MYOS_ELOOP ? ELOOP : ENOTDIR;
        return -1;
    }
    if (ret == (long)MYOS_EINTR) {
        errno = EINTR; /* blocking FIFO open interrupted by a caught signal */
        return -1;
    }
    if (ret == (long)MYOS_ENXIO) {
        errno = ENXIO; /* FIFO: O_WRONLY|O_NONBLOCK and no reader */
        return -1;
    }
    if (failed(ret)) {
        /* No controlling terminal → ENXIO (Linux open(/dev/tty) semantics);
         * a directory opened to write, create or truncate: EISDIR; another
         * file the caller can see but not open so: the policy refused it
         * (docs/security.md). */
        if (strcmp(path, "/dev/tty") == 0) {
            errno = ENXIO;
        } else if (fstatat(dirfd, path, &st, 0) != 0) {
            errno = ENOENT;
        } else if (S_ISDIR(st.st_mode) && ((flags & O_ACCMODE) != O_RDONLY || (flags & (O_CREAT | O_TRUNC)))) {
            errno = EISDIR;
        } else {
            errno = EACCES;
        }
        return -1;
    }
    /* Only FIFOs get userspace O_NONBLOCK reads from open(): other paths
     * (ttys, ptys, files) keep their historical blocking behaviour that
     * dropbear/curl rely on; fcntl(F_SETFL) still sets it anywhere. */
    if ((flags & O_NONBLOCK) && fstatat((int)ret, "", &st, AT_EMPTY_PATH) == 0 && S_ISFIFO(st.st_mode)) {
        myos_fd_nonblock_set((int)ret, 1);
    }
    return (int)ret;
}

int _open(const char *path, int flags, ...) {
    return openat(AT_FDCWD, path, flags);
}

/* ---- making and removing names ------------------------------------------- */

/* Why making `path` (relative to `dirfd`) failed: something is there
 * already, its directory is missing, or the filesystem refused. */
static int make_errno(int dirfd, const char *path, int refused) {
    struct stat st;
    char dir[MYOS_MAX_PATH + 1];
    const char *slash;
    size_t n;
    if (fstatat(dirfd, path, &st, AT_SYMLINK_NOFOLLOW) == 0) {
        return EEXIST;
    }
    slash = strrchr(path, '/');
    if (slash == NULL) {
        return refused;
    }
    n = slash == path ? 1 : (size_t)(slash - path);
    memcpy(dir, path, n);
    dir[n] = '\0';
    return fstatat(dirfd, dir, &st, 0) == 0 ? refused : ENOENT;
}

int mkdirat(int dirfd, const char *path, mode_t mode) {
    long len = k_len_named(path);
    (void)mode;
    if (len < 0) {
        return -1;
    }
    if (failed(myos_syscall6(MYOS_SYS_MKNODAT, k_dirfd(dirfd), (long)(uintptr_t)path, len, MYOS_MKNOD_DIR, 0, 0))) {
        /* `mkdir -p` tolerates EEXIST only. */
        errno = make_errno(dirfd, path, EROFS);
        return -1;
    }
    return 0;
}

int mkdir(const char *path, mode_t mode) {
    return mkdirat(AT_FDCWD, path, mode);
}

int _mkdir(const char *path, mode_t mode) {
    return mkdir(path, mode);
}

/* Named pipes live on tmpfs (/tmp); other mounts refuse them. */
int mkfifoat(int dirfd, const char *path, mode_t mode) {
    long len = k_len_named(path);
    (void)mode;
    if (len < 0) {
        return -1;
    }
    if (failed(myos_syscall6(MYOS_SYS_MKNODAT, k_dirfd(dirfd), (long)(uintptr_t)path, len, MYOS_MKNOD_FIFO, 0, 0))) {
        errno = make_errno(dirfd, path, EPERM);
        return -1;
    }
    return 0;
}

int mkfifo(const char *path, mode_t mode) {
    return mkfifoat(AT_FDCWD, path, mode);
}

int _mkfifo(const char *path, mode_t mode) {
    return mkfifo(path, mode);
}

/* Only FIFOs can be created; device nodes live in the kernel's devfs. */
int mknodat(int dirfd, const char *path, mode_t mode, dev_t dev) {
    (void)dev;
    if (S_ISFIFO(mode)) {
        return mkfifoat(dirfd, path, mode & 07777);
    }
    errno = EPERM;
    return -1;
}

int mknod(const char *path, mode_t mode, dev_t dev) {
    return mknodat(AT_FDCWD, path, mode, dev);
}

int _mknod(const char *path, mode_t mode, dev_t dev) {
    return mknod(path, mode, dev);
}

int symlinkat(const char *target, int dirfd, const char *path) {
    long tlen = k_len_named(target);
    long len = k_len_named(path);
    if (tlen < 0 || len < 0) {
        return -1;
    }
    if (failed(myos_syscall6(MYOS_SYS_SYMLINKAT, (long)(uintptr_t)target, tlen, k_dirfd(dirfd),
                             (long)(uintptr_t)path, len, 0))) {
        errno = make_errno(dirfd, path, EROFS);
        return -1;
    }
    return 0;
}

int symlink(const char *target, const char *path) {
    return symlinkat(target, AT_FDCWD, path);
}

int _symlink(const char *target, const char *path) {
    return symlink(target, path);
}

int unlinkat(int dirfd, const char *path, int flags) {
    struct stat st;
    long len = k_len_named(path);
    if (len < 0) {
        return -1;
    }
    if (failed(myos_syscall6(MYOS_SYS_UNLINKAT, k_dirfd(dirfd), (long)(uintptr_t)path, len, k_flags(flags), 0, 0))) {
        if (fstatat(dirfd, path, &st, AT_SYMLINK_NOFOLLOW) != 0) {
            errno = ENOENT;
        } else if (flags & AT_REMOVEDIR) {
            errno = S_ISDIR(st.st_mode) ? ENOTEMPTY : ENOTDIR;
        } else {
            errno = S_ISDIR(st.st_mode) ? EISDIR : EACCES;
        }
        return -1;
    }
    return 0;
}

int _unlink(const char *path) {
    return unlinkat(AT_FDCWD, path, 0);
}

int rmdir(const char *path) {
    return unlinkat(AT_FDCWD, path, AT_REMOVEDIR);
}

int renameat(int olddirfd, const char *oldpath, int newdirfd, const char *newpath) {
    struct stat st;
    long olen = k_len_named(oldpath);
    long nlen = k_len_named(newpath);
    if (olen < 0 || nlen < 0) {
        return -1;
    }
    if (failed(myos_syscall6(MYOS_SYS_RENAMEAT, k_dirfd(olddirfd), (long)(uintptr_t)oldpath, olen,
                             k_dirfd(newdirfd), (long)(uintptr_t)newpath, nlen))) {
        errno = fstatat(olddirfd, oldpath, &st, AT_SYMLINK_NOFOLLOW) == 0 ? EACCES : ENOENT;
        return -1;
    }
    return 0;
}

int _rename(const char *oldpath, const char *newpath) {
    return renameat(AT_FDCWD, oldpath, AT_FDCWD, newpath);
}

ssize_t readlinkat(int dirfd, const char *path, char *buf, size_t size) {
    long len = k_len_named(path);
    long ret;
    if (len < 0) {
        return -1;
    }
    if (buf == NULL || size == 0) {
        errno = EINVAL;
        return -1;
    }
    ret = myos_syscall6(MYOS_SYS_READLINKAT, k_dirfd(dirfd), (long)(uintptr_t)path, len,
                        (long)(uintptr_t)buf, (long)size, 0);
    if (failed(ret)) {
        struct stat st;
        errno = fstatat(dirfd, path, &st, AT_SYMLINK_NOFOLLOW) == 0 ? EINVAL : ENOENT;
        return -1;
    }
    return (ssize_t)ret;
}

ssize_t readlink(const char *path, char *buf, size_t size) {
    return readlinkat(AT_FDCWD, path, buf, size);
}

/* ---- times --------------------------------------------------------------- */

/* timespec pair (NULL = both now) -> the kernel's two int64_t seconds. */
static int times_from_timespec(const struct timespec ts[2], int64_t out[2]) {
    for (int i = 0; i < 2; i++) {
        if (ts[i].tv_nsec == UTIME_NOW) {
            out[i] = MYOS_UTIME_NOW;
        } else if (ts[i].tv_nsec == UTIME_OMIT) {
            out[i] = MYOS_UTIME_OMIT;
        } else if (ts[i].tv_nsec < 0 || ts[i].tv_nsec >= 1000000000L || ts[i].tv_sec < 0) {
            errno = EINVAL;
            return -1;
        } else {
            out[i] = (int64_t)ts[i].tv_sec; /* the kernel keeps seconds */
        }
    }
    return 0;
}

int utimensat(int dirfd, const char *path, const struct timespec times[2], int flags) {
    struct stat st;
    int64_t t[2];
    long len;
    if (times != NULL && times_from_timespec(times, t) < 0) {
        return -1;
    }
    /* Linux: a NULL path is the fd itself. */
    if (path == NULL) {
        path = "";
        flags |= AT_EMPTY_PATH;
    }
    len = (flags & AT_EMPTY_PATH) ? k_len(path) : k_len_named(path);
    if (len < 0) {
        return -1;
    }
    if (failed(myos_syscall6(MYOS_SYS_UTIMENSAT, k_dirfd(dirfd), (long)(uintptr_t)path, len,
                             (long)(uintptr_t)(times != NULL ? t : NULL), k_flags(flags), 0))) {
        /* A missing file, or a filesystem that keeps no times. */
        errno = fstatat(dirfd, path, &st, flags) == 0 ? EROFS : len == 0 ? EBADF : ENOENT;
        return -1;
    }
    return 0;
}

int futimens(int fd, const struct timespec times[2]) {
    return utimensat(fd, "", times, AT_EMPTY_PATH);
}

/* ---- the cwd ------------------------------------------------------------- */

static int chdirat(int dirfd, const char *path, int flags) {
    struct stat st;
    long len = (flags & AT_EMPTY_PATH) ? k_len(path) : k_len_named(path);
    if (len < 0) {
        return -1;
    }
    if (failed(myos_syscall6(MYOS_SYS_CHDIRAT, k_dirfd(dirfd), (long)(uintptr_t)path, len, k_flags(flags), 0, 0))) {
        if (fstatat(dirfd, path, &st, flags) != 0) {
            errno = len == 0 ? EBADF : ENOENT;
        } else {
            errno = S_ISDIR(st.st_mode) ? EACCES : ENOTDIR;
        }
        return -1;
    }
    return 0;
}

int chdir(const char *path) {
    return chdirat(AT_FDCWD, path, 0);
}

int fchdir(int fd) {
    return chdirat(fd, "", AT_EMPTY_PATH);
}

char *getcwd(char *buf, size_t size) {
    if (buf == NULL || size == 0) {
        errno = EINVAL;
        return NULL;
    }
    if (failed(myos_syscall3(MYOS_SYS_GETCWD, (long)(uintptr_t)buf, (long)size, 0))) {
        /* Too small a buffer, or a cwd that has been removed. */
        errno = size < MYOS_MAX_PATH + 1 ? ERANGE : ENOENT;
        return NULL;
    }
    return buf;
}

/* chroot: a namespace of one binding, `path` at `/` (docs/security.md). */
int chroot(const char *path) {
    struct stat st;
    char spec[MYOS_MAX_PATH + 16];
    long len = k_len_named(path);
    if (len < 0) {
        return -1;
    }
    if (stat(path, &st) != 0) {
        errno = ENOENT;
        return -1;
    }
    if (!S_ISDIR(st.st_mode)) {
        errno = ENOTDIR;
        return -1;
    }
    if (strchr(path, ' ') != NULL || strchr(path, '\n') != NULL) {
        errno = EINVAL; /* the binding line cannot hold it */
        return -1;
    }
    strcpy(spec, "/ ");
    strcat(spec, path);
    strcat(spec, " all\n");
    return myos_ns(spec);
}

/* ---- directories --------------------------------------------------------- */

long myos_listdirat(int dirfd, const char *path, char *buf, size_t cap, int flags) {
    long len = (flags & AT_EMPTY_PATH) ? k_len(path) : k_len_named(path);
    long ret;
    if (len < 0) {
        return -1;
    }
    ret = myos_syscall6(MYOS_SYS_LISTDIRAT, k_dirfd(dirfd), (long)(uintptr_t)path, len, (long)(uintptr_t)buf,
                        (long)cap, k_flags(flags));
    if (failed(ret)) {
        struct stat st;
        if (fstatat(dirfd, path, &st, flags) != 0) {
            errno = len == 0 ? EBADF : ENOENT;
        } else {
            errno = S_ISDIR(st.st_mode) ? EACCES : ENOTDIR;
        }
        return -1;
    }
    return ret;
}

/* ---- exec ---------------------------------------------------------------- */

/* Kernel exec limits (kernel/src/user/mod.rs MAX_ARGC, MAX_ENVC,
 * MAX_EXEC_STRINGS): the counts, and the bytes of all strings with their
 * NULs. */
#define MYOS_MAX_ARGC 1024
#define MYOS_MAX_ENVC 1024
#define MYOS_MAX_EXEC_STRINGS (128 * 1024)

/* The native exec block: argc, (pointer, length) per argument, envc, the
 * same per variable, then the strings. */
static unsigned long *exec_pack(char *const argv[], char *const envp[]) {
    size_t bytes = 0;
    size_t argc = 0;
    size_t envc = 0;
    size_t i;
    unsigned long *pack;
    char *store;

    for (; argv != NULL && argv[argc] != NULL; argc++) {
        bytes += strlen(argv[argc]) + 1;
    }
    for (; envp != NULL && envp[envc] != NULL; envc++) {
        bytes += strlen(envp[envc]) + 1;
    }
    if (argc > MYOS_MAX_ARGC || envc > MYOS_MAX_ENVC || bytes > MYOS_MAX_EXEC_STRINGS) {
        errno = E2BIG;
        return NULL;
    }
    pack = malloc((2 + 2 * (argc + envc)) * sizeof(unsigned long) + bytes);
    if (pack == NULL) {
        errno = ENOMEM;
        return NULL;
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
    return pack;
}

static int execat(int dirfd, const char *path, char *const argv[], char *const envp[], int flags) {
    struct stat st;
    unsigned long *pack;
    long len = (flags & AT_EMPTY_PATH) ? k_len(path) : k_len_named(path);
    if (len < 0) {
        return -1;
    }
    pack = exec_pack(argv, envp);
    if (pack == NULL) {
        return -1;
    }
    myos_syscall6(MYOS_SYS_EXECAT, k_dirfd(dirfd), (long)(uintptr_t)path, len, (long)(uintptr_t)pack,
                  k_flags(flags), 0);
    /* Only reached on failure: a missing file, or one the caller may not
     * run (never ENOEXEC, on which a shell runs the file as a script). */
    free(pack);
    errno = fstatat(dirfd, path, &st, flags) != 0 ? ENOENT : EACCES;
    return -1;
}

int _execve(const char *path, char *const argv[], char *const envp[]) {
    return execat(AT_FDCWD, path, argv, envp, 0);
}

int fexecve(int fd, char *const argv[], char *const envp[]) {
    return execat(fd, "", argv, envp, AT_EMPTY_PATH);
}
