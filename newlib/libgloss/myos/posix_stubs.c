/* myos libgloss: POSIX helpers sbase expects beyond read-only VFS hooks. */

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <pwd.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int myos_rofs(void) {
    errno = EROFS;
    return -1;
}

static int myos_nosys(void) {
    errno = ENOSYS;
    return -1;
}

int access(const char *path, int mode) {
    struct stat st;
    (void)mode;
    if (path == NULL) {
        errno = EINVAL;
        return -1;
    }
    if (stat(path, &st) < 0) {
        return -1;
    }
    return 0;
}

int creat(const char *path, mode_t mode) {
    (void)path;
    (void)mode;
    return myos_rofs();
}

int chmod(const char *path, mode_t mode) {
    (void)path;
    (void)mode;
    return myos_rofs();
}

int mkdir(const char *path, mode_t mode) {
    (void)path;
    (void)mode;
    return myos_rofs();
}

mode_t umask(mode_t mask) {
    static mode_t cur = 022;
    mode_t old = cur;
    cur = mask;
    return old;
}

int symlink(const char *target, const char *linkpath) {
    (void)target;
    (void)linkpath;
    return myos_rofs();
}

int mknod(const char *path, mode_t mode, dev_t dev) {
    (void)path;
    (void)mode;
    (void)dev;
    return myos_rofs();
}

int chown(const char *path, uid_t owner, gid_t group) {
    (void)path;
    (void)owner;
    (void)group;
    return myos_rofs();
}

int lchown(const char *path, uid_t owner, gid_t group) {
    (void)path;
    (void)owner;
    (void)group;
    return myos_rofs();
}

int openat(int dirfd, const char *path, int flags, ...) {
    (void)dirfd;
    (void)path;
    (void)flags;
    return myos_nosys();
}

int faccessat(int dirfd, const char *path, int mode, int flags) {
    (void)dirfd;
    (void)flags;
    return access(path, mode);
}

int fstatat(int dirfd, const char *path, struct stat *st, int flags) {
    (void)dirfd;
    (void)flags;
    if (st == NULL) {
        errno = EINVAL;
        return -1;
    }
    return stat(path, st);
}

int unlinkat(int dirfd, const char *path, int flags) {
    (void)dirfd;
    (void)path;
    (void)flags;
    return myos_rofs();
}

int utimensat(int dirfd, const char *path, const struct timespec times[2], int flags) {
    (void)dirfd;
    (void)path;
    (void)times;
    (void)flags;
    return myos_rofs();
}

DIR *fdopendir(int fd) {
    (void)fd;
    errno = ENOSYS;
    return NULL;
}

int execvp(const char *file, char *const argv[]) {
    (void)file;
    (void)argv;
    return myos_nosys();
}

pid_t waitpid(pid_t pid, int *status, int options) {
    (void)pid;
    (void)status;
    (void)options;
    errno = ECHILD;
    return -1;
}

long sysconf(int name) {
    (void)name;
    errno = ENOSYS;
    return -1;
}

unsigned sleep(unsigned seconds) {
    (void)seconds;
    return 0;
}

uid_t getuid(void) {
    return 0;
}

gid_t getgid(void) {
    return 0;
}

int dup2(int oldfd, int newfd) {
    (void)oldfd;
    (void)newfd;
    return myos_nosys();
}

int fchownat(int dirfd, const char *path, uid_t owner, gid_t group, int flags) {
    (void)dirfd;
    (void)path;
    (void)owner;
    (void)group;
    (void)flags;
    return myos_rofs();
}

int symlinkat(const char *target, int dirfd, const char *path) {
    (void)target;
    (void)dirfd;
    (void)path;
    return myos_rofs();
}

int setpriority(int which, id_t who, int prio) {
    (void)which;
    (void)who;
    (void)prio;
    return myos_nosys();
}

int setsid(void) {
    return myos_nosys();
}

struct group *getgrnam(const char *name) {
    (void)name;
    errno = ENOENT;
    return NULL;
}

struct passwd *getpwnam(const char *name) {
    (void)name;
    errno = ENOENT;
    return NULL;
}

int _mkdir(const char *path, mode_t mode) {
    return mkdir(path, mode);
}

int _chmod(const char *path, mode_t mode) {
    return chmod(path, mode);
}

int _access(const char *path, int mode) {
    return access(path, mode);
}

int _creat(const char *path, mode_t mode) {
    return creat(path, mode);
}

mode_t _umask(mode_t mask) {
    return umask(mask);
}

int _symlink(const char *target, const char *linkpath) {
    return symlink(target, linkpath);
}
