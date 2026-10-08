/*
 * Shared file mappings (kernel/src/fs/pagecache.rs, user/aspace.rs,
 * docs/linux-compat.md): a MAP_SHARED mapping of a regular file through
 * a writable fd is the file. What is stored through it reads back with
 * read() at once, what write() puts in the file shows in it, a second
 * mapping and a forked child write the same pages, a read() into it lands
 * in the file (the kernel's copy), it survives munmap, close and the exit
 * of a child that never unmapped, and msync writes it back and returns 0.
 * ftruncate growth gives it zero pages that write through; a cut zeros
 * what it cut off. A mapping made PROT_READ turns writable with mprotect;
 * a MAP_PRIVATE mapping stays a copy. Through a read-only fd a MAP_SHARED
 * mapping reads the file (the writes of the others included) and refuses
 * PROT_WRITE. A 16-page pattern goes through and comes back whole.
 *
 * `mmap_smoke [DIR]` works in DIR (default /tmp): the ext2 test runs it on
 * the scratch disk.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define PAGE 4096
#define PAGES 3

static int failures;
static char dir[128];

static void check(int ok, const char *what) {
    if (!ok) {
        printf("mmap_smoke: %s (errno %d)\n", what, errno);
        failures++;
    }
}

static const char *in_dir(const char *name) {
    static char path[192];
    snprintf(path, sizeof path, "%s/%s", dir, name);
    return path;
}

static unsigned char pattern(size_t i) {
    return (unsigned char)(i * 7 + 3);
}

/* A file of `pages` pages of the pattern, open read-write. */
static int make_file(const char *name, int pages) {
    int fd = open(name, O_RDWR | O_CREAT | O_TRUNC, 0644);
    unsigned char buf[PAGE];
    check(fd >= 0, "create the file");
    for (int p = 0; p < pages; p++) {
        for (int i = 0; i < PAGE; i++) {
            buf[i] = pattern(p * PAGE + i);
        }
        check(write(fd, buf, PAGE) == PAGE, "fill the file");
    }
    return fd;
}

static int byte_at(int fd, off_t off) {
    unsigned char c;
    return pread(fd, &c, 1, off) == 1 ? c : -1;
}

static void shared(void) {
    const char *f = in_dir("mmap_shared");
    int fd = make_file(f, PAGES);
    unsigned char *p, *q, *priv;
    int second, other, status;
    pid_t pid;
    char buf[8], other_path[192];

    p = mmap(NULL, PAGES * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    check(p != MAP_FAILED, "MAP_SHARED mapping of a file");
    if (p == MAP_FAILED) {
        close(fd);
        return;
    }
    check(p[100] == pattern(100) && p[2 * PAGE + 1] == pattern(2 * PAGE + 1), "the mapping reads the file");

    /* A store reads back through the file at once, and a write to the
     * file shows in the mapping. */
    p[100] = 'X';
    p[PAGE + 5] = 'Y';
    check(byte_at(fd, 100) == 'X' && byte_at(fd, PAGE + 5) == 'Y', "a store shows in read()");
    check(pwrite(fd, "hello", 5, 200) == 5, "pwrite into the mapped file");
    check(memcmp(p + 200, "hello", 5) == 0, "write() shows in the mapping");
    check(pwrite(fd, "span", 4, PAGE - 2) == 4, "pwrite across a page boundary");
    check(memcmp(p + PAGE - 2, "span", 4) == 0, "a write across pages shows in the mapping");

    /* A second mapping of the file is the same pages. */
    second = open(f, O_RDWR);
    q = mmap(NULL, PAGES * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, second, 0);
    check(q != MAP_FAILED, "a second MAP_SHARED mapping");
    if (q != MAP_FAILED) {
        check(q[100] == 'X' && memcmp(q + 200, "hello", 5) == 0, "the second mapping sees the stores");
        q[300] = 'Q';
        check(p[300] == 'Q' && byte_at(fd, 300) == 'Q', "a store through the second mapping shows in both");
        check(munmap(q, PAGES * PAGE) == 0, "munmap the second mapping");
    }
    close(second);

    /* A forked child writes the same pages, with and without munmap. */
    pid = fork();
    if (pid == 0) {
        p[2 * PAGE + 1] = 'Z';
        _exit(p[100] == 'X' ? 0 : 1);
    }
    check(pid > 0 && waitpid(pid, &status, 0) == pid && status == 0, "the child sees the parent's store");
    check(p[2 * PAGE + 1] == 'Z' && byte_at(fd, 2 * PAGE + 1) == 'Z', "the child's store shows in the parent and the file");
    pid = fork();
    if (pid == 0) {
        int cfd = open(f, O_RDWR);
        unsigned char *c = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, cfd, PAGE);
        if (c == MAP_FAILED) {
            _exit(1);
        }
        c[7] = 'E';
        _exit(0);
    }
    check(pid > 0 && waitpid(pid, &status, 0) == pid && status == 0, "a child maps page 1 and exits");
    check(p[PAGE + 7] == 'E' && byte_at(fd, PAGE + 7) == 'E', "a store of a child that exited shows");

    /* The kernel's copy into the mapping (a read() of another file). */
    snprintf(other_path, sizeof other_path, "%s/mmap_other", dir);
    other = open(other_path, O_RDWR | O_CREAT | O_TRUNC, 0644);
    check(other >= 0 && write(other, "kernel", 6) == 6, "another file");
    check(pread(other, p + 400, 6, 0) == 6, "read() into the mapping");
    check(memcmp(p + 400, "kernel", 6) == 0 && pread(fd, buf, 6, 400) == 6 && memcmp(buf, "kernel", 6) == 0,
          "what read() put in the mapping is in the file");
    close(other);
    unlink(other_path);

    /* A private mapping is a copy: its stores reach neither. */
    priv = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
    check(priv != MAP_FAILED, "a MAP_PRIVATE mapping of the file");
    if (priv != MAP_FAILED) {
        check(priv[100] == 'X', "the private mapping reads the file");
        priv[100] = 'P';
        check(p[100] == 'X' && byte_at(fd, 100) == 'X', "a private store stays private");
        p[101] = 'S';
        check(priv[101] != 'S' || priv[100] == 'P', "the private copy is the process's own");
        check(munmap(priv, PAGE) == 0, "munmap the private mapping");
    }

    check(msync(p, PAGES * PAGE, MS_SYNC) == 0, "msync");
    check(msync(p, PAGES * PAGE, MS_ASYNC | MS_INVALIDATE) == 0, "msync MS_ASYNC");
    check(msync(p, PAGES * PAGE, MS_ASYNC | MS_SYNC) < 0 && errno == EINVAL, "msync refuses both");

    /* A cut zeros what it cut off; growth gives zero pages that write through. */
    p[PAGE + 200] = 'T';
    check(ftruncate(fd, PAGE + 100) == 0, "ftruncate to a page and a bit");
    check(p[PAGE + 5] == 'Y' && p[PAGE + 200] == 0, "the cut page is zero past the end");
    check(ftruncate(fd, PAGES * PAGE) == 0, "ftruncate back");
    check(byte_at(fd, PAGE + 200) == 0 && p[PAGE + 200] == 0, "grown with zeros, in the file and the mapping");
    p[PAGE + 200] = 'G';
    check(byte_at(fd, PAGE + 200) == 'G', "a store after the growth shows in read()");

    /* munmap and close: the stores are in the file. */
    check(munmap(p, PAGES * PAGE) == 0, "munmap");
    close(fd);
    fd = open(f, O_RDONLY);
    check(fd >= 0 && byte_at(fd, 100) == 'X' && byte_at(fd, PAGE + 5) == 'Y' && byte_at(fd, 300) == 'Q'
              && byte_at(fd, PAGE + 7) == 'E' && byte_at(fd, PAGE + 200) == 'G',
          "the stores survive munmap and close");
    check(byte_at(fd, 0) == pattern(0) && byte_at(fd, PAGE + 50) == pattern(PAGE + 50), "the rest is as it was");
    check(byte_at(fd, 2 * PAGE + 1) == 0 && byte_at(fd, 2 * PAGE + 2) == 0, "the cut page is zero in the file");
    close(fd);
    unlink(f);
}

static void protections(void) {
    const char *f = in_dir("mmap_prot");
    int fd = make_file(f, 1);
    int ro = open(f, O_RDONLY);
    unsigned char *p, *r;

    p = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, fd, 0);
    check(p != MAP_FAILED, "a PROT_READ MAP_SHARED mapping");
    r = mmap(NULL, PAGE, PROT_READ, MAP_SHARED, ro, 0);
    check(r != MAP_FAILED, "MAP_SHARED through a read-only fd");
    check(mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, ro, 0) == MAP_FAILED,
          "PROT_WRITE MAP_SHARED through a read-only fd is refused");
    if (p != MAP_FAILED) {
        check(mprotect(p, PAGE, PROT_READ | PROT_WRITE) == 0, "mprotect to writable");
        p[10] = 'W';
        check(byte_at(fd, 10) == 'W', "a store after mprotect shows in read()");
        if (r != MAP_FAILED) {
            check(r[10] == 'W', "the read-only fd's mapping sees it");
        }
        check(munmap(p, PAGE) == 0, "munmap");
    }
    if (r != MAP_FAILED) {
        check(munmap(r, PAGE) == 0, "munmap the read-only one");
    }
    close(ro);
    close(fd);
    unlink(f);
}

static void whole(void) {
    const char *f = in_dir("mmap_whole");
    int pages = 16, fd, bad = 0;
    unsigned char *p, buf[PAGE];

    fd = open(f, O_RDWR | O_CREAT | O_TRUNC, 0644);
    check(fd >= 0 && ftruncate(fd, pages * PAGE) == 0, "an empty file of 16 pages");
    p = mmap(NULL, pages * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    check(p != MAP_FAILED, "map it");
    if (p == MAP_FAILED) {
        close(fd);
        return;
    }
    for (size_t i = 0; i < (size_t)pages * PAGE; i++) {
        p[i] = pattern(i);
    }
    check(munmap(p, pages * PAGE) == 0, "munmap");
    for (int page = 0; page < pages; page++) {
        if (pread(fd, buf, PAGE, page * PAGE) != PAGE) {
            bad++;
            continue;
        }
        for (int i = 0; i < PAGE; i++) {
            if (buf[i] != pattern(page * PAGE + i)) {
                bad++;
                break;
            }
        }
    }
    check(bad == 0, "the 16 pages came back whole");
    close(fd);
    unlink(f);
}

int main(int argc, char **argv) {
    snprintf(dir, sizeof dir, "%s", argc > 1 ? argv[1] : "/tmp");
    shared();
    protections();
    whole();
    return failures != 0;
}
