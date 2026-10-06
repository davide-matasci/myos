/*
 * The *at calls (kernel/src/user/at.rs, libgloss at.c): a directory fd
 * stands for its directory whatever is renamed meanwhile, and so does the
 * cwd; an fd's own file is there for fstat after an unlink; fdopendir lists
 * a directory fd; stat follows a symlink and lstat does not.
 *
 * `at_smoke cap` and `at_smoke capro` run in a namespace that cannot name
 * fd 3, a directory holding `f` ("in") and `sub` (user/tests/kernel.sh):
 * the fd is a capability, opened with every right (cap) or read only
 * (capro). Paths resolve beneath it only, with its rights.
 */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/myos_extra.h> /* lstat */
#include <sys/stat.h>
#include <unistd.h>

static int failures;

static void check(int ok, const char *what) {
    if (!ok) {
        printf("at_smoke: %s (errno %d)\n", what, errno);
        failures++;
    }
}

static int exists(const char *path) {
    struct stat st;
    return lstat(path, &st) == 0;
}

static int has_line(int dirfd, const char *name) {
    int fd = openat(dirfd, ".", O_RDONLY);
    DIR *d = fd >= 0 ? fdopendir(fd) : NULL;
    struct dirent *e;
    int seen = 0;
    while (d != NULL && (e = readdir(d)) != NULL) {
        seen |= strcmp(e->d_name, name) == 0;
    }
    if (d != NULL) {
        closedir(d);
    }
    return seen;
}

static int cap(int writable) {
    struct stat st;
    char buf[16];
    int fd, sub;

    fd = openat(3, "f", O_RDONLY);
    check(fd >= 0 && read(fd, buf, sizeof buf) == 3 && memcmp(buf, "in\n", 3) == 0, "read f beneath the fd");
    check(fd >= 0 && fstat(fd, &st) == 0 && (writable ? (st.st_mode & 0200) != 0 : (st.st_mode & 0777) == 0400),
          "f's rights");
    close(fd);
    check(fstat(3, &st) == 0 && S_ISDIR(st.st_mode), "fstat the fd");
    check(has_line(3, "sub"), "list the fd");
    check(openat(3, "/tmp/sec/cap/f", O_RDONLY) < 0, "no absolute name for it");

    /* Nothing above it, from it or from a directory beneath it. */
    check(openat(3, "..", O_RDONLY) < 0, "no .. above the fd");
    check(openat(3, "sub/../../cap/f", O_RDONLY) < 0, "no way out and back in");
    check(openat(3, "sub/../f", O_RDONLY) >= 0, ".. beneath the fd");
    sub = openat(3, "sub", O_RDONLY);
    check(sub >= 0 && openat(sub, "../f", O_RDONLY) < 0, "no .. above a directory beneath");

    /* No name in the caller's view: neither the cwd nor a program. */
    check(fchdir(3) < 0, "no fchdir into the fd");

    if (writable) {
        /* Symlinks resolve beneath it too: up and out, or absolute, is out. */
        check(symlinkat("sub/../f", 3, "in") == 0, "symlinkat in");
        check(symlinkat("../disk-b", 3, "up") == 0, "symlinkat up");
        check(symlinkat("/tmp/sec/cap/f", 3, "abs") == 0, "symlinkat abs");
        check(fstatat(3, "in", &st, 0) == 0 && S_ISREG(st.st_mode), "follow a link beneath");
        check(fstatat(3, "up", &st, 0) < 0 && fstatat(3, "up", &st, AT_SYMLINK_NOFOLLOW) == 0, "no link out");
        check(fstatat(3, "abs", &st, 0) < 0, "no absolute link");
        fd = openat(sub, "g", O_WRONLY | O_CREAT, 0644);
        check(fd >= 0 && write(fd, "g", 1) == 1, "create beneath a directory beneath");
        close(fd);
        check(renameat(sub, "g", 3, "g") == 0 && unlinkat(3, "g", 0) == 0, "renameat, unlinkat");
        check(unlinkat(3, "in", 0) == 0 && unlinkat(3, "up", 0) == 0 && unlinkat(3, "abs", 0) == 0, "unlink the links");
    } else {
        check(openat(3, "g", O_WRONLY | O_CREAT, 0644) < 0, "no create in a read-only fd");
        check(openat(3, "f", O_WRONLY) < 0, "no write in a read-only fd");
        check(unlinkat(3, "f", 0) < 0, "no unlink in a read-only fd");
        check(mkdirat(sub, "d", 0755) < 0, "no mkdir beneath a read-only fd");
    }
    close(sub);
    return failures != 0;
}

int main(int argc, char **argv) {
    struct stat st;
    char buf[64];
    int dir, file;
    ssize_t n;

    if (argc > 1) {
        return cap(strcmp(argv[1], "cap") == 0);
    }
    mkdir("/tmp/ats", 0755);
    check(mkdir("/tmp/ats/d", 0755) == 0, "mkdir d");
    dir = open("/tmp/ats/d", O_RDONLY);
    check(dir >= 0, "open d");

    /* The directory moves; the fd follows it, a new d is another one. */
    check(rename("/tmp/ats/d", "/tmp/ats/e") == 0, "rename d e");
    check(mkdir("/tmp/ats/d", 0755) == 0, "mkdir new d");
    file = openat(dir, "f", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    check(file >= 0, "openat f");
    check(write(file, "hello", 5) == 5, "write f");
    check(exists("/tmp/ats/e/f") && !exists("/tmp/ats/d/f"), "f made in e, not in the new d");
    check(fstatat(dir, "f", &st, 0) == 0 && st.st_size == 5, "fstatat f");

    /* fstat of an unlinked file is the fd's own. */
    check(unlinkat(dir, "f", 0) == 0, "unlinkat f");
    check(fstat(file, &st) == 0 && S_ISREG(st.st_mode) && st.st_size == 5, "fstat after unlink");
    close(file);

    /* Names relative to the fd. */
    check(mkdirat(dir, "sub", 0755) == 0, "mkdirat sub");
    check(symlinkat("sub", dir, "link") == 0, "symlinkat link");
    n = readlinkat(dir, "link", buf, sizeof buf);
    check(n == 3 && memcmp(buf, "sub", 3) == 0, "readlinkat link");
    check(fstatat(dir, "link", &st, AT_SYMLINK_NOFOLLOW) == 0 && S_ISLNK(st.st_mode), "lstat link is a link");
    check(fstatat(dir, "link", &st, 0) == 0 && S_ISDIR(st.st_mode), "stat link is the directory");
    check(renameat(dir, "link", dir, "link2") == 0 && exists("/tmp/ats/e/link2"), "renameat link");
    check(unlinkat(dir, "link2", 0) == 0, "unlinkat link2");

    /* fdopendir lists the fd's directory. */
    {
        int dup_fd = openat(dir, ".", O_RDONLY);
        DIR *d = dup_fd >= 0 ? fdopendir(dup_fd) : NULL;
        struct dirent *e;
        int saw_sub = 0;
        check(d != NULL, "fdopendir");
        while (d != NULL && (e = readdir(d)) != NULL) {
            saw_sub |= strcmp(e->d_name, "sub") == 0;
        }
        check(saw_sub, "readdir sees sub");
        if (d != NULL) {
            closedir(d);
        }
    }
    check(unlinkat(dir, "sub", AT_REMOVEDIR) == 0, "unlinkat sub");

    /* The cwd is a directory too: it follows a rename. */
    check(fchdir(dir) == 0, "fchdir");
    check(getcwd(buf, sizeof buf) != NULL && strcmp(buf, "/tmp/ats/e") == 0, "getcwd e");
    check(rename("/tmp/ats/e", "/tmp/ats/x") == 0, "rename e x");
    check(getcwd(buf, sizeof buf) != NULL && strcmp(buf, "/tmp/ats/x") == 0, "getcwd follows to x");
    file = open("g", O_WRONLY | O_CREAT, 0644);
    check(file >= 0 && exists("/tmp/ats/x/g"), "relative open in the moved cwd");
    close(file);
    check(chdir("/") == 0, "chdir /");

    close(dir);
    unlink("/tmp/ats/x/g");
    rmdir("/tmp/ats/x");
    rmdir("/tmp/ats/d");
    rmdir("/tmp/ats");
    return failures != 0;
}
