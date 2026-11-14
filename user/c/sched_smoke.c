/* sched-smoke: boot-CI guest test of the scheduler's idle-pull balancing
 * (docs/scheduler.md).
 *
 * A forked child starts on its parent's home CPU. While the parent keeps
 * that CPU busy, an idle CPU takes the child and makes itself its home: a
 * while later the two have different homes (/proc/<pid>/task/<tid>/status,
 * the cpu field). Needs two CPUs that run user tasks: all of them on
 * riscv64, every one but the first elsewhere; with fewer it only prints
 * that. Prints [ OK ] sched.
 *
 * `sched_smoke shootdown`: a task whose wait cannot begin still answers a
 * TLB shootdown (docs/scheduler.md "TLB shootdown"). A child polls six
 * wait sets of a hundred idle event counters each, forever, while a thread
 * of this process keeps a pipe busy. Every pipe operation counts as a
 * wake and a look at 600 counters takes longer than the next one is in
 * coming, so the child's wait returns before it began, pass after pass,
 * in the kernel, in one syscall.
 * Meanwhile this process maps, touches and unmaps a page in a loop: with
 * its thread on another CPU each unmap is a shootdown (riscv64, x86_64;
 * aarch64's `tlbi` broadcasts), which waits for every running CPU, the
 * child's among them. Answered from the child's loop, an unmap takes one
 * pass of it longer than without the child; unanswered, each waited out
 * the shootdown's spin cap, a thousand times an unmap's time and more.
 * With netd as the poller (a second per shootdown, its poll's timeout)
 * the riscv64 packages boot stood still for 39 s (CI #2604). Prints
 * [ OK ] shootdown.
 *
 * `sched_smoke spin`: such a poller's CPU keeps its tick (issue #788). The
 * same child, the same thread; this process reads the child's CPU time
 * two hundred times, 10 ms apart. A running task's time is charged at
 * each tick of its CPU, so it lags by a tick at most (docs/proc.md): the
 * child's moves from one reading to the next. Looping with interrupts
 * off, its CPU took no tick, and its time stood still until something
 * else passed through that CPU's scheduler, half a second at a time; so
 * did whatever else waits for a tick there: the preemption of the loop,
 * the stuck-CPU watch (a poller ten seconds in that loop was a stuck
 * CPU), and on riscv64, where CPU 0 runs user tasks, every device's
 * interrupt. Prints [ OK ] spin.
 */
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static int fail(const char *what) {
    printf("[ FAIL ] sched %s\n", what);
    return 1;
}

static long now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000L + ts.tv_nsec / 1000000L;
}

/* Spin for `ms` without a syscall: the CPU stays busy with this task. */
static void busy(long ms) {
    long end = now_ms() + ms;
    volatile unsigned long n = 0;
    while (now_ms() < end) {
        for (int i = 0; i < 10000; i++) {
            n++;
        }
    }
}

/* The home CPU of thread `tid` of process `pid` (-1: none), and its CPU
 * time in ms: the cpu and time fields of its status line
 * (docs/proc.md). */
static int status(pid_t pid, pid_t tid, int *cpu, long *time_ms) {
    char path[64], line[256], name[32], state[16], cpu_s[8], wait[32];
    snprintf(path, sizeof path, "/proc/%d/task/%d/status", (int)pid, (int)tid);
    FILE *f = fopen(path, "r");
    if (!f || !fgets(line, sizeof line, f)) {
        if (f) {
            fclose(f);
        }
        return -1;
    }
    fclose(f);
    int t, p;
    if (sscanf(line, "%d %d %31s %15s %7s %31s %ld", &t, &p, name, state, cpu_s, wait, time_ms) != 7) {
        return -1;
    }
    *cpu = cpu_s[0] == '-' ? -1 : atoi(cpu_s);
    return 0;
}

/* How many CPUs there are, and whether user tasks run on CPU 0 (riscv64:
 * every CPU; elsewhere the first keeps the console and the kernel's own
 * loop). */
static int user_cpus(void) {
    FILE *f = fopen("/proc/cpuinfo", "r");
    char line[128];
    int count = 0, on_bsp = 0;
    if (!f) {
        return 0;
    }
    while (fgets(line, sizeof line, f)) {
        if (sscanf(line, "processor_count: %d", &count) == 1) {
            continue;
        }
        if (!strncmp(line, "arch: riscv64", 13)) {
            on_bsp = 1;
        }
    }
    fclose(f);
    return on_bsp ? count : count - 1;
}

/* The poller's wait sets and the event counters in each (600 looks a
 * pass: most of a millisecond under emulation, where the thread's pipe is
 * written or read fifty times in that; counters, as the system has 64
 * pipes), how long the unmaps are counted without the poller and beside
 * it, and how much slower they may be beside it: a few times where each
 * is answered within a pass, hundreds of times and more where each waits
 * for the spin cap. */
#define SHOOT_SETS 6
#define SHOOT_EVENTS 100
#define SHOOT_ALONE_MS 500
#define SHOOT_MS 2000
#define SHOOT_SLOWER 50

static volatile int traffic_stop;

/* Pipe traffic until told to stop: a byte in, a byte out. */
static void *traffic(void *arg) {
    int *p = arg;
    char c = 0;
    while (!traffic_stop) {
        if (write(p[1], &c, 1) != 1 || read(p[0], &c, 1) != 1) {
            break;
        }
    }
    return NULL;
}

/* Map, touch and unmap a page for `ms`: how often, or -1. */
static long unmaps(long ms) {
    long cycles = 0, end = now_ms() + ms;
    while (now_ms() < end) {
        char *m = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (m == MAP_FAILED) {
            return -1;
        }
        m[0] = 1;
        if (munmap(m, 4096) != 0) {
            return -1;
        }
        cycles++;
    }
    return cycles;
}

/* A poller that cannot sleep while `traffic` runs: a child that polls
 * SHOOT_SETS wait sets of SHOOT_EVENTS idle event counters each, forever,
 * once a byte is written to `*go`. Forked before this process has a
 * thread, and back once its sets are there: they take a while (an open
 * and a write a member). Its pid, or -1. */
static pid_t poller_start(int *go) {
    int ready[2], start[2];
    char c = 0;
    if (pipe(ready) != 0 || pipe(start) != 0) {
        return -1;
    }
    pid_t poller = fork();
    if (poller < 0) {
        return -1;
    }
    if (poller == 0) {
        static struct pollfd fds[SHOOT_SETS];
        static int events[SHOOT_EVENTS];
        for (int i = 0; i < SHOOT_EVENTS; i++) {
            events[i] = eventfd(0, 0);
            if (events[i] < 0) {
                _exit(2);
            }
        }
        for (int s = 0; s < SHOOT_SETS; s++) {
            int ep = epoll_create1(0);
            if (ep < 0) {
                _exit(4);
            }
            for (int i = 0; i < SHOOT_EVENTS; i++) {
                struct epoll_event ev = {.events = EPOLLIN};
                if (epoll_ctl(ep, EPOLL_CTL_ADD, events[i], &ev) != 0) {
                    _exit(5);
                }
            }
            fds[s].fd = ep;
            fds[s].events = POLLIN;
        }
        if (write(ready[1], &c, 1) != 1 || read(start[0], &c, 1) != 1) {
            _exit(6);
        }
        poll(fds, SHOOT_SETS, -1);
        _exit(3);
    }
    close(ready[1]);
    close(start[0]);
    if (read(ready[0], &c, 1) != 1) {
        waitpid(poller, NULL, 0);
        return -1;
    }
    close(ready[0]);
    *go = start[1];
    return poller;
}

/* End the poller: 0 if it was still in its poll. */
static int poller_stop(pid_t poller) {
    int status = 0;
    kill(poller, SIGKILL);
    waitpid(poller, &status, 0);
    if (!WIFSIGNALED(status)) {
        printf("the poller left by itself (status %#x)\n", status);
        return -1;
    }
    return 0;
}

static int shootdown(void) {
    static int p[2];
    int go;
    char c = 0;
    if (pipe(p) != 0) {
        return fail("shootdown: pipe");
    }
    pid_t poller = poller_start(&go);
    if (poller < 0) {
        return fail("shootdown: the poller's wait sets");
    }
    pthread_t t;
    if (pthread_create(&t, NULL, traffic, p) != 0) {
        poller_stop(poller);
        return fail("shootdown: pthread_create");
    }
    /* The thread gets a CPU of its own; then the unmaps without the
     * poller, the poller into its poll, and the unmaps beside it. */
    struct timespec settle = {0, 200 * 1000 * 1000};
    nanosleep(&settle, NULL);
    long alone = unmaps(SHOOT_ALONE_MS);
    long beside = -1;
    if (write(go, &c, 1) == 1) {
        nanosleep(&settle, NULL);
        beside = unmaps(SHOOT_MS);
    }
    traffic_stop = 1;
    pthread_join(t, NULL);
    if (poller_stop(poller) != 0) {
        return fail("shootdown: the poller did not stay in its poll");
    }
    if (alone <= 0 || beside < 0) {
        return fail("shootdown: mmap, munmap");
    }
    printf("%ld map/unmap cycles in %d ms, %ld in %d ms beside a poller that cannot sleep\n", alone, SHOOT_ALONE_MS,
           beside, SHOOT_MS);
    if (beside * SHOOT_ALONE_MS * SHOOT_SLOWER < alone * SHOOT_MS) {
        return fail("shootdown: the unmaps waited for the poller's CPU");
    }
    printf("[ OK ] shootdown\n");
    return 0;
}

/* How often the poller's CPU time is read, how far apart (a tick), and in
 * how many of the readings it must have moved: nearly all of them where
 * its CPU ticks (193 to 200 under emulation; a host that stops the CPU's
 * thread for a while costs some), one in ten where it does not (10 to
 * 24). */
#define SPIN_READS 200
#define SPIN_APART_MS 10
#define SPIN_MIN 100

static int spin(void) {
    static int p[2];
    int go;
    char c = 0;
    if (pipe(p) != 0) {
        return fail("spin: pipe");
    }
    pid_t poller = poller_start(&go);
    if (poller < 0) {
        return fail("spin: the poller's wait sets");
    }
    pthread_t t;
    if (pthread_create(&t, NULL, traffic, p) != 0) {
        poller_stop(poller);
        return fail("spin: pthread_create");
    }
    /* This thread stays on its CPU from here on, busy between the
     * readings: asleep, it woke on the CPU it had last, the poller's as
     * likely as not, and each wake was a pass through that CPU's scheduler
     * (a task queued for the poller's CPU is run from its loop). */
    int moved = 0, cpu, bad = write(go, &c, 1) != 1;
    long before = 0, now = 0, first = 0, flat = 0, longest = 0, since = 0;
    busy(200);
    bad = bad || status(poller, poller, &cpu, &before) < 0;
    first = before;
    since = now_ms();
    for (int i = 0; !bad && i < SPIN_READS; i++) {
        busy(SPIN_APART_MS);
        bad = status(poller, poller, &cpu, &now) < 0;
        if (now > before) {
            moved++;
            since = now_ms();
        }
        flat = now_ms() - since;
        if (flat > longest) {
            longest = flat;
        }
        before = now;
    }
    traffic_stop = 1;
    pthread_join(t, NULL);
    if (poller_stop(poller) != 0) {
        return fail("spin: the poller did not stay in its poll");
    }
    if (bad) {
        return fail("spin: the poller's status");
    }
    printf("the poller's CPU time moved in %d of %d readings %d ms apart (%ld ms in all), stood still for %ld ms at most\n",
           moved, SPIN_READS, SPIN_APART_MS, now - first, longest);
    if (moved < SPIN_MIN) {
        return fail("spin: the poller's CPU took no tick");
    }
    printf("[ OK ] spin\n");
    return 0;
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "shootdown") == 0) {
        return shootdown();
    }
    if (argc > 1 && strcmp(argv[1], "spin") == 0) {
        return spin();
    }
    int cpus = user_cpus();
    if (cpus < 2) {
        printf("[ OK ] sched (%d CPU for user tasks: nothing to balance)\n", cpus);
        return 0;
    }
    pid_t me = getpid();
    int home, child_home;
    long t, child_t;
    if (status(me, me, &home, &t) < 0) {
        return fail("read own status");
    }
    pid_t child = fork();
    if (child < 0) {
        return fail("fork");
    }
    if (child == 0) {
        busy(10000);
        _exit(0);
    }
    /* The parent keeps its CPU: the child, queued there, is taken by an
     * idle one; then both run. */
    busy(500);
    int rc = status(child, child, &child_home, &child_t);
    int mine = status(me, me, &home, &t);
    kill(child, SIGKILL);
    waitpid(child, NULL, 0);
    if (rc < 0 || mine < 0) {
        return fail("read the child's status");
    }
    printf("parent on cpu %d (%ld ms), child on cpu %d (%ld ms)\n", home, t, child_home, child_t);
    if (child_home == home) {
        return fail("the child shares the busy parent's CPU");
    }
    if (child_t < 100) {
        return fail("the child barely ran");
    }
    printf("[ OK ] sched\n");
    return 0;
}
