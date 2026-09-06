/* myos libgloss: remember open() paths so *at(dirfd, rel) can join correctly.
 *
 * Kernel open/stat are path-based; there is no openat/fstatat syscall yet.
 * Userspace which/test/ln open a PATH directory then fstatat(dirfd, name).
 * Without this table those *at stubs ignored dirfd and looked in cwd.
 */

#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <unistd.h>

#include "myos_syscalls.h"

#ifndef AT_FDCWD
#define AT_FDCWD (-100)
#endif

#define MYOS_FD_PATH_MAX 32
#define MYOS_PATH_BUF 512

static char fd_paths[MYOS_FD_PATH_MAX][MYOS_PATH_BUF];
static unsigned char fd_path_set[MYOS_FD_PATH_MAX];

void myos_fd_path_set(int fd, const char *path) {
    size_t n;

    if (fd < 0 || fd >= MYOS_FD_PATH_MAX || path == NULL) {
        return;
    }
    n = strlen(path);
    if (n == 0 || n >= MYOS_PATH_BUF) {
        fd_path_set[fd] = 0;
        return;
    }
    memcpy(fd_paths[fd], path, n + 1);
    fd_path_set[fd] = 1;
}

void myos_fd_path_clear(int fd) {
    if (fd < 0 || fd >= MYOS_FD_PATH_MAX) {
        return;
    }
    fd_path_set[fd] = 0;
}

void myos_fd_path_dup(int oldfd, int newfd) {
    if (newfd < 0 || newfd >= MYOS_FD_PATH_MAX) {
        return;
    }
    if (oldfd >= 0 && oldfd < MYOS_FD_PATH_MAX && fd_path_set[oldfd]) {
        memcpy(fd_paths[newfd], fd_paths[oldfd], MYOS_PATH_BUF);
        fd_path_set[newfd] = 1;
    } else {
        fd_path_set[newfd] = 0;
    }
}

int myos_fd_path_resolve(int dirfd, const char *path, char *out, size_t outsz) {
    const char *dir;
    size_t dlen;
    size_t plen;
    size_t need;
    int slash;

    if (path == NULL || out == NULL || outsz == 0) {
        errno = EINVAL;
        return -1;
    }
    plen = strlen(path);
    if (plen == 0) {
        errno = ENOENT;
        return -1;
    }

    /* Absolute path: dirfd is ignored. */
    if (path[0] == '/') {
        if (plen + 1 > outsz) {
            errno = ENAMETOOLONG;
            return -1;
        }
        memcpy(out, path, plen + 1);
        return 0;
    }

    /* AT_FDCWD: relative to process cwd — pass through for path-based syscalls. */
    if (dirfd == AT_FDCWD) {
        if (plen + 1 > outsz) {
            errno = ENAMETOOLONG;
            return -1;
        }
        memcpy(out, path, plen + 1);
        return 0;
    }

    if (dirfd < 0 || dirfd >= MYOS_FD_PATH_MAX || !fd_path_set[dirfd]) {
        errno = EBADF;
        return -1;
    }
    dir = fd_paths[dirfd];
    dlen = strlen(dir);
    slash = (dlen > 0 && dir[dlen - 1] != '/');
    need = dlen + (size_t)slash + plen + 1;
    if (need > outsz) {
        errno = ENAMETOOLONG;
        return -1;
    }
    memcpy(out, dir, dlen);
    if (slash) {
        out[dlen++] = '/';
    }
    memcpy(out + dlen, path, plen + 1);
    return 0;
}
