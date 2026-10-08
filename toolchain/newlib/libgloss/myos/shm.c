/* myos libgloss: POSIX shared memory, memfd_create and System V shared
 * memory, as files of /dev/shm (a directory of the tmpfs, bound there by
 * the kernel). A shm_open fd is close-on-exec, as POSIX has it; a memfd is
 * the file created and unlinked at once: it lives while an fd or a mapping
 * holds it, and a MAP_SHARED mapping of it is shared with a forked child.
 *
 * A System V segment is the file /dev/shm/sysv.k<key> (IPC_PRIVATE: a
 * name of this process and a counter), its id the file's inode number
 * (unique until reboot): shmat finds the file by it among the directory's
 * entries and maps it MAP_SHARED, shmctl(IPC_RMID) unlinks it (the
 * attached mappings go on sharing it; no new attach after that), and
 * IPC_STAT reports its size and a mode of 0666 (the file's protection is
 * the policy's: the X server, without the client's credentials, checks
 * the mode's "other" bits). */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/ipc.h>
#include <sys/mman.h>
#include <sys/shm.h>
#include <sys/stat.h>
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

/* ---- System V shared memory ------------------------------------------- */

#define SYSV_PREFIX "sysv."
#define SYSV_SEGMENTS 64

/* The segments this process has attached. */
static struct {
    void *addr;
    size_t size;
} attached[SYSV_SEGMENTS];

/* The file of segment `id`: its path in `path`, its stat in `st`. */
static int sysv_find(int id, char *path, size_t n, struct stat *st) {
    DIR *d = opendir(SHM_DIR);
    struct dirent *e;
    if (d == NULL) {
        errno = EINVAL;
        return -1;
    }
    while ((e = readdir(d)) != NULL) {
        if (strncmp(e->d_name, SYSV_PREFIX, sizeof SYSV_PREFIX - 1) != 0) {
            continue;
        }
        if (snprintf(path, n, SHM_DIR "%s", e->d_name) >= (int)n) {
            continue;
        }
        if (stat(path, st) == 0 && (int)st->st_ino == id) {
            closedir(d);
            return 0;
        }
    }
    closedir(d);
    errno = EINVAL;
    return -1;
}

key_t ftok(const char *path, int id) {
    struct stat st;
    if (stat(path, &st) < 0) {
        return (key_t)-1;
    }
    return (key_t)(((unsigned)id & 0xff) << 24 | ((unsigned)st.st_dev & 0xff) << 16 | ((unsigned)st.st_ino & 0xffff));
}

int shmget(key_t key, size_t size, int shmflg) {
    static unsigned seq;
    char path[256];
    struct stat st;
    int fd, flags = O_RDWR;
    if (key == IPC_PRIVATE) {
        snprintf(path, sizeof path, SHM_DIR SYSV_PREFIX "p%ld.%u", (long)getpid(), seq++);
        flags |= O_CREAT | O_EXCL;
    } else {
        snprintf(path, sizeof path, SHM_DIR SYSV_PREFIX "k%ld", (long)key);
        if (shmflg & IPC_CREAT) {
            flags |= O_CREAT;
        }
        if (shmflg & IPC_EXCL) {
            flags |= O_EXCL;
        }
    }
    fd = open(path, flags | O_CLOEXEC, 0600);
    if (fd < 0) {
        return -1;
    }
    if (fstat(fd, &st) < 0) {
        close(fd);
        return -1;
    }
    if (st.st_size == 0 && size > 0) {
        /* New (or still empty): sized now. */
        if (ftruncate(fd, (off_t)size) < 0) {
            close(fd);
            return -1;
        }
    } else if ((size_t)st.st_size < size) {
        /* An existing segment smaller than asked for. */
        close(fd);
        errno = EINVAL;
        return -1;
    }
    close(fd);
    return (int)st.st_ino;
}

void *shmat(int shmid, const void *shmaddr, int shmflg) {
    char path[256];
    struct stat st;
    int fd, i, rdonly = shmflg & SHM_RDONLY;
    void *p;
    if (shmaddr != NULL) {
        /* An address of the caller's choosing: not supported. */
        errno = EINVAL;
        return (void *)-1;
    }
    if (sysv_find(shmid, path, sizeof path, &st) < 0) {
        return (void *)-1;
    }
    for (i = 0; i < SYSV_SEGMENTS && attached[i].addr != NULL; i++) {
    }
    if (i == SYSV_SEGMENTS) {
        errno = EMFILE;
        return (void *)-1;
    }
    fd = open(path, (rdonly ? O_RDONLY : O_RDWR) | O_CLOEXEC);
    if (fd < 0) {
        return (void *)-1;
    }
    p = mmap(NULL, (size_t)st.st_size, rdonly ? PROT_READ : PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    close(fd);
    if (p == MAP_FAILED) {
        return (void *)-1;
    }
    attached[i].addr = p;
    attached[i].size = (size_t)st.st_size;
    return p;
}

int shmdt(const void *shmaddr) {
    int i;
    for (i = 0; i < SYSV_SEGMENTS; i++) {
        if (attached[i].addr == shmaddr && shmaddr != NULL) {
            int rc = munmap(attached[i].addr, attached[i].size);
            attached[i].addr = NULL;
            attached[i].size = 0;
            return rc;
        }
    }
    errno = EINVAL;
    return -1;
}

int shmctl(int shmid, int cmd, struct shmid_ds *buf) {
    char path[256];
    struct stat st;
    if (sysv_find(shmid, path, sizeof path, &st) < 0) {
        return -1;
    }
    switch (cmd) {
    case IPC_STAT:
        if (buf == NULL) {
            errno = EFAULT;
            return -1;
        }
        memset(buf, 0, sizeof *buf);
        buf->shm_perm.uid = buf->shm_perm.cuid = getuid();
        buf->shm_perm.gid = buf->shm_perm.cgid = getgid();
        buf->shm_perm.mode = 0666;
        buf->shm_segsz = (size_t)st.st_size;
        buf->shm_atime = buf->shm_dtime = st.st_atime;
        buf->shm_ctime = st.st_mtime;
        buf->shm_cpid = buf->shm_lpid = getpid();
        buf->shm_nattch = 1;
        return 0;
    case IPC_SET:
        /* The permissions are the policy's: nothing to set. */
        return 0;
    case IPC_RMID:
        return unlink(path);
    default:
        errno = EINVAL;
        return -1;
    }
}
