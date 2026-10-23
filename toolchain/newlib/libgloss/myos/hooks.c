/* newlib's hooks: the _NAME functions its libc calls for NAME (read, open,
 * fork, ...), each the call of toolchain/libmyos. Nothing else of libgloss
 * is newlib's own interface: the rest of libmyos is exported under its
 * public names by the aliases of its files (libmyos/libc.h). */

#include <_ansi.h>
#include <errno.h>
#include <fcntl.h>
#include <reent.h>
#include <stddef.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/times.h>
#include <sys/types.h>
#include <unistd.h>

#include "libmyos.h"

int _close(int fd) {
    return __myos_close(fd);
}

void _exit(int status) {
    __myos_exit(status);
}

int _read(int fd, void *buf, size_t cnt) {
    return (int)__myos_read(fd, buf, cnt);
}

int _write(int fd, const void *buf, size_t cnt) {
    return (int)__myos_write(fd, buf, cnt);
}

off_t _lseek(int fd, off_t pos, int whence) {
    return __myos_lseek(fd, pos, whence);
}

int _isatty(int fd) {
    if (myos_fd_is_tty(fd)) {
        return 1;
    }
    errno = ENOTTY;
    return 0;
}

void *_sbrk(ptrdiff_t incr) {
    return __myos_sbrk(incr);
}

int _getpid(void) {
    return __myos_getpid();
}

int _kill(int pid, int sig) {
    return __myos_kill(pid, sig);
}

int _fork(void) {
    return __myos_fork();
}

int _wait(int *status) {
    return __myos_wait(status);
}

int _execve(const char *path, char *const argv[], char *const envp[]) {
    return __myos_execve(path, argv, envp);
}

int _gettimeofday(struct timeval *tv, void *tz) {
    return __myos_gettimeofday(tv, tz);
}

clock_t _times(struct tms *buf) {
    (void)buf;
    errno = ENOSYS;
    return (clock_t)-1;
}

int _stat(const char *path, struct stat *st) {
    return __myos_stat(path, st);
}

int _lstat(const char *path, struct stat *st) {
    return __myos_lstat(path, st);
}

int _fstat(int fd, struct stat *st) {
    return __myos_fstat(fd, st);
}

int _access(const char *path, int mode) {
    return __myos_access(path, mode);
}

int _open(const char *path, int flags, ...) {
    return __myos_openat(AT_FDCWD, path, flags);
}

int _mkdir(const char *path, mode_t mode) {
    return __myos_mkdir(path, mode);
}

int _mkfifo(const char *path, mode_t mode) {
    return __myos_mkfifo(path, mode);
}

int _mknod(const char *path, mode_t mode, dev_t dev) {
    return __myos_mknod(path, mode, dev);
}

int _symlink(const char *target, const char *path) {
    return __myos_symlink(target, path);
}

int _unlink(const char *path) {
    return __myos_unlink(path);
}

int _rename(const char *oldpath, const char *newpath) {
    return __myos_rename(oldpath, newpath);
}

int _link(const char *oldpath, const char *newpath) {
    return __myos_link(oldpath, newpath);
}

int _chown(const char *path, uid_t owner, gid_t group) {
    return __myos_chown(path, owner, group);
}

int _chmod(const char *path, mode_t mode) {
    return __myos_chmod(path, mode);
}

int _pipe(int fildes[2]) {
    return __myos_pipe(fildes);
}

/* newlib libc exports fcntl() when HAVE_FCNTL; `wide` is the whole
 * argument word (toolchain/newlib/patch.sh patch_fcntl_arg). */
int _fcntl(int fd, int cmd, long wide) {
    return __myos_fcntl(fd, cmd, wide);
}
