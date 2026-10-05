/*
 * The *at calls (kernel/src/user/at.rs, libgloss at.c): a directory fd
 * stands for its directory whatever is renamed meanwhile, and so does the
 * cwd; an fd's own file is there for fstat after an unlink; fdopendir lists
 * a directory fd; stat follows a symlink and lstat does not.
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

int main(void) {
    struct stat st;
    char buf[64];
    int dir, file;
    ssize_t n;

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
