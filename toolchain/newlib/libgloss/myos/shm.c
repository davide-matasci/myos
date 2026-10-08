/* myos libgloss: POSIX shared memory and memfd_create, as files of
 * /dev/shm (a directory of the tmpfs, bound there by the kernel). A
 * shm_open fd is close-on-exec, as POSIX has it; a memfd is the file
 * created and unlinked at once: it lives while an fd or a mapping holds
 * it, and a MAP_SHARED mapping of it is shared with a forked child. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define SHM_DIR "/dev/shm/"

/* `/name` or `name`, one component, as the path of its file. */
static int shm_path(const char *name, char *path, size_t n) {
    while (*name == '/') {
        name++;
    }
    if (*name == '\0' || strchr(name, '/') != NULL) {
        errno = EINVAL;
        return -1;
    }
    if (snprintf(path, n, SHM_DIR "%s", name) >= (int)n) {
        errno = ENAMETOOLONG;
        return -1;
    }
    return 0;
}

int shm_open(const char *name, int oflag, mode_t mode) {
    char path[256];
    if (shm_path(name, path, sizeof path) < 0) {
        return -1;
    }
    return open(path, oflag | O_CLOEXEC, mode);
}

int shm_unlink(const char *name) {
    char path[256];
    if (shm_path(name, path, sizeof path) < 0) {
        return -1;
    }
    return unlink(path);
}

int memfd_create(const char *name, unsigned flags) {
    static unsigned seq;
    char path[256];
    int tries;
    if (flags & ~(MFD_CLOEXEC | MFD_ALLOW_SEALING)) {
        errno = EINVAL;
        return -1;
    }
    for (tries = 0; tries < 100; tries++) {
        int fd;
        if (snprintf(path, sizeof path, SHM_DIR "memfd:%s.%ld.%u", name, (long)getpid(), seq++) >= (int)sizeof path) {
            errno = ENAMETOOLONG;
            return -1;
        }
        fd = open(path, O_RDWR | O_CREAT | O_EXCL | ((flags & MFD_CLOEXEC) ? O_CLOEXEC : 0), 0600);
        if (fd >= 0) {
            unlink(path);
            return fd;
        }
        if (errno != EEXIST) {
            return -1;
        }
    }
    errno = EEXIST;
    return -1;
}
