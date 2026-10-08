/*
 * Shared memory (libgloss shm.c, kernel/src/fs/vfs.rs `anon_file`,
 * docs/linux-compat.md): shm_open makes a file of /dev/shm that a forked
 * child opens by name and maps with the parent, shm_unlink takes the name
 * away while the mappings go on sharing; a memfd (created and unlinked at
 * once) is written through its fd and its mapping, shared with a forked
 * child and with a program the child execs (the fd passes exec without
 * MFD_CLOEXEC, not with it); an anonymous MAP_SHARED mapping is zero,
 * shared with a forked child, and MAP_PRIVATE is not; a System V segment
 * (libgloss shm.c, over a file of /dev/shm) is attached by its id in a
 * forked child, IPC_STAT tells its size, a keyed one is found again by its
 * key and refused with IPC_EXCL, IPC_RMID takes it away while the attached
 * mappings go on sharing; /dev/shm has no leftover entry once the fds and
 * mappings are gone.
 *
 * `shm_smoke child FD`: the exec'd program, maps the memfd FD and answers.
 */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ipc.h>
#include <sys/mman.h>
#include <sys/shm.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define PAGE 4096

static int failures;

static void check(int ok, const char *what) {
    if (!ok) {
        printf("shm_smoke: %s (errno %d)\n", what, errno);
        failures++;
    }
}

static int entries(const char *dir) {
    DIR *d = opendir(dir);
    struct dirent *e;
    int n = 0;
    if (d == NULL) {
        return -1;
    }
    while ((e = readdir(d)) != NULL) {
        if (strcmp(e->d_name, ".") != 0 && strcmp(e->d_name, "..") != 0) {
            n++;
        }
    }
    closedir(d);
    return n;
}

static int wait_ok(pid_t pid) {
    int status;
    return pid > 0 && waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0;
}

/* The exec'd child: the memfd is page 0 = "parent", answers "child". */
static int child(const char *fdarg) {
    int fd = atoi(fdarg);
    char *p = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (p == MAP_FAILED || strcmp(p, "parent") != 0) {
        return 1;
    }
    strcpy(p + 64, "child");
    return 0;
}

static void posix_shm(void) {
    const char *name = "/shm_smoke";
    int fd, fd2;
    char *p;
    pid_t pid;

    shm_unlink(name);
    fd = shm_open(name, O_RDWR | O_CREAT | O_EXCL, 0600);
    check(fd >= 0, "shm_open creates");
    check(fd >= 0 && fcntl(fd, F_GETFD) == FD_CLOEXEC, "a shm_open fd is close-on-exec");
    check(shm_open(name, O_RDWR | O_CREAT | O_EXCL, 0600) < 0 && errno == EEXIST, "O_EXCL on the name");
    check(ftruncate(fd, 2 * PAGE) == 0, "ftruncate the segment");
    p = mmap(NULL, 2 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    check(p != MAP_FAILED, "map the segment");
    if (p == MAP_FAILED) {
        close(fd);
        shm_unlink(name);
        return;
    }
    strcpy(p, "hello");
    pid = fork();
    if (pid == 0) {
        /* By name, in another process: the same pages. */
        int cfd = shm_open(name, O_RDWR, 0);
        char *c = cfd < 0 ? MAP_FAILED : mmap(NULL, 2 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, cfd, 0);
        if (c == MAP_FAILED || strcmp(c, "hello") != 0) {
            _exit(1);
        }
        strcpy(c + PAGE, "from the child");
        _exit(0);
    }
    check(wait_ok(pid), "the child opened and read the segment");
    check(strcmp(p + PAGE, "from the child") == 0, "the child's store is in the parent's mapping");
    check(shm_unlink(name) == 0, "shm_unlink");
    check(shm_open(name, O_RDWR, 0) < 0 && errno == ENOENT, "the name is gone");
    /* The segment lives on for its holders: a second fd on the
     * unlinked file still reads what the mapping writes. */
    fd2 = dup(fd);
    strcpy(p + 100, "after unlink");
    {
        char buf[16] = {0};
        check(pread(fd2, buf, 12, 100) == 12 && strcmp(buf, "after unlink") == 0, "the unlinked segment is still shared");
    }
    close(fd2);
    check(munmap(p, 2 * PAGE) == 0 && close(fd) == 0, "munmap and close");
    check(shm_open("", O_RDWR, 0) < 0 && errno == EINVAL, "an empty name");
    check(shm_open("a/b", O_RDWR | O_CREAT, 0600) < 0 && errno == EINVAL, "a name with a slash");
}

static void memfd(void) {
    int fd, keep, status;
    char *p, buf[16] = {0}, fdarg[16];
    pid_t pid;

    fd = memfd_create("smoke", MFD_CLOEXEC);
    check(fd >= 0, "memfd_create");
    if (fd < 0) {
        return;
    }
    check(fcntl(fd, F_GETFD) == FD_CLOEXEC, "MFD_CLOEXEC");
    check(ftruncate(fd, PAGE) == 0 && pwrite(fd, "fd", 3, 10) == 3, "ftruncate and pwrite the memfd");
    p = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    check(p != MAP_FAILED, "map the memfd");
    if (p == MAP_FAILED) {
        close(fd);
        return;
    }
    check(strcmp(p + 10, "fd") == 0, "the mapping reads what pwrite put");
    strcpy(p, "parent");
    check(pread(fd, buf, 7, 0) == 7 && strcmp(buf, "parent") == 0, "read() sees the store");
    pid = fork();
    if (pid == 0) {
        strcpy(p + 32, "forked");
        _exit(strcmp(p, "parent") == 0 ? 0 : 1);
    }
    check(wait_ok(pid), "the forked child shares the memfd");
    check(strcmp(p + 32, "forked") == 0, "the forked child's store");
    /* Through exec: a dup without close-on-exec passes, the original not. */
    keep = dup(fd);
    snprintf(fdarg, sizeof fdarg, "%d", keep);
    pid = fork();
    if (pid == 0) {
        execl("/bin/etc/shm_smoke", "shm_smoke", "child", fdarg, (char *)NULL);
        _exit(127);
    }
    check(wait_ok(pid), "the exec'd child maps the memfd by its fd");
    check(strcmp(p + 64, "child") == 0, "the exec'd child's store");
    snprintf(fdarg, sizeof fdarg, "%d", fd);
    pid = fork();
    if (pid == 0) {
        execl("/bin/etc/shm_smoke", "shm_smoke", "child", fdarg, (char *)NULL);
        _exit(127);
    }
    check(pid > 0 && waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 1,
          "the close-on-exec memfd is gone in the exec'd child");
    close(keep);
    check(munmap(p, PAGE) == 0 && close(fd) == 0, "munmap and close the memfd");
    check(memfd_create("x", 0x100) < 0 && errno == EINVAL, "an unknown memfd flag");
}

static void anonymous(void) {
    char *p, *q;
    pid_t pid;

    p = mmap(NULL, 2 * PAGE, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANON, -1, 0);
    check(p != MAP_FAILED, "MAP_SHARED | MAP_ANON");
    if (p == MAP_FAILED) {
        return;
    }
    check(p[0] == 0 && p[PAGE + 7] == 0, "zero");
    q = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    check(q != MAP_FAILED, "MAP_PRIVATE | MAP_ANON");
    strcpy(p, "parent");
    pid = fork();
    if (pid == 0) {
        strcpy(p + PAGE, "child");
        if (q != MAP_FAILED) {
            strcpy(q, "private");
        }
        _exit(strcmp(p, "parent") == 0 ? 0 : 1);
    }
    check(wait_ok(pid), "the child sees the parent's store");
    check(strcmp(p + PAGE, "child") == 0, "the child's store is shared");
    check(q == MAP_FAILED || q[0] == 0, "a private mapping's store is the child's own");
    check(msync(p, 2 * PAGE, MS_SYNC) == 0, "msync an anonymous mapping");
    check(munmap(p, 2 * PAGE) == 0, "munmap");
    if (q != MAP_FAILED) {
        munmap(q, PAGE);
    }
    /* A second mapping, after the first went: new and zero. */
    p = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANON, -1, 0);
    check(p != MAP_FAILED && p[0] == 0, "another anonymous shared mapping is zero");
    if (p != MAP_FAILED) {
        munmap(p, PAGE);
    }
}

static void sysv(void) {
    struct shmid_ds ds;
    int id, keyed, status;
    char *p;
    pid_t pid;

    id = shmget(IPC_PRIVATE, 2 * PAGE, IPC_CREAT | 0600);
    check(id >= 0, "shmget IPC_PRIVATE");
    if (id < 0) {
        return;
    }
    p = shmat(id, NULL, 0);
    check(p != (void *)-1, "shmat");
    if (p == (void *)-1) {
        shmctl(id, IPC_RMID, NULL);
        return;
    }
    check(p[0] == 0 && p[PAGE] == 0, "a new segment is zero");
    check(shmctl(id, IPC_STAT, &ds) == 0 && ds.shm_segsz == 2 * PAGE && (ds.shm_perm.mode & 0666) == 0666,
          "IPC_STAT");
    strcpy(p, "parent");
    pid = fork();
    if (pid == 0) {
        char *c = shmat(id, NULL, 0);
        if (c == (void *)-1 || strcmp(c, "parent") != 0) {
            _exit(1);
        }
        strcpy(c + PAGE, "child");
        _exit(shmdt(c) == 0 ? 0 : 2);
    }
    check(pid > 0 && waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0,
          "a forked child attaches the segment by its id");
    check(strcmp(p + PAGE, "child") == 0, "the child's store is in the segment");
    check(shmctl(id, IPC_RMID, NULL) == 0, "IPC_RMID");
    check(shmat(id, NULL, 0) == (void *)-1 && errno == EINVAL, "no attach after IPC_RMID");
    strcpy(p + 100, "still");
    check(strcmp(p + 100, "still") == 0, "the attached mapping lives on");
    check(shmdt(p) == 0, "shmdt");
    check(shmdt(p) < 0 && errno == EINVAL, "shmdt of nothing");

    keyed = shmget(1234, PAGE, IPC_CREAT | IPC_EXCL | 0600);
    check(keyed >= 0, "shmget by key");
    check(shmget(1234, PAGE, IPC_CREAT | IPC_EXCL | 0600) < 0 && errno == EEXIST, "IPC_EXCL on a key in use");
    check(shmget(1234, PAGE, 0) == keyed, "the key names the segment");
    check(shmget(1234, 2 * PAGE, 0) < 0 && errno == EINVAL, "asking for more than the segment");
    check(shmctl(keyed, IPC_RMID, NULL) == 0, "IPC_RMID of the keyed segment");
    check(shmget(1234, PAGE, 0) < 0 && errno == ENOENT, "the key is free again");
    check(shmget(5678, PAGE, 0) < 0 && errno == ENOENT, "an unknown key without IPC_CREAT");
}

int main(int argc, char **argv) {
    int before;
    if (argc == 3 && strcmp(argv[1], "child") == 0) {
        return child(argv[2]);
    }
    before = entries("/dev/shm");
    check(before >= 0, "/dev/shm is a directory");
    posix_shm();
    memfd();
    anonymous();
    sysv();
    check(entries("/dev/shm") == before, "/dev/shm has no leftover entry");
    return failures != 0;
}
