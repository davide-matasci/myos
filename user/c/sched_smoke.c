/* sched-smoke: boot-CI guest test of the scheduler's idle-pull balancing
 * (docs/pci-acpi-smp.md, "Scheduler").
 *
 * A forked child starts on its parent's home CPU. While the parent keeps
 * that CPU busy, an idle CPU takes the child and makes itself its home: a
 * while later the two have different homes (/proc/<pid>/task/<tid>/status,
 * the cpu field). Needs two CPUs that run user tasks: all of them on
 * riscv64, every one but the first elsewhere; with fewer it only prints
 * that. Prints [ OK ] sched.
 */
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
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

int main(void) {
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
