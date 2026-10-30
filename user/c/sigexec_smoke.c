/* sigexec-smoke: a signal's disposition across fork and exec, as POSIX has
 * it and a shell relies on it for its jobs.
 *
 * A background job of a shell without job control ignores SIGINT and
 * SIGQUIT: the shell's forked child sets SIG_IGN before the exec, and the
 * kernel keeps an ignored signal ignored across the exec (a caught one
 * reverts to its default, since the handler is gone with the old image;
 * docs/signals.md). `sleep 3 & kill -INT $!` then leaves the sleep alive.
 * Each case forks a child that execs `sleep`, waits until the exec has
 * happened (/proc/<pid>/status names the program), sends it a signal and
 * looks whether it survived:
 *
 *   ign   the child sets SIG_IGN on SIGINT before the exec: the sleep
 *         survives the SIGINT (a SIGTERM then ends it);
 *   dfl   the child leaves the parent's handler on SIGINT in place: the
 *         exec resets it to the default, and the SIGINT ends the sleep;
 *   inh   the parent itself ignores SIGINT when it forks, the child changes
 *         nothing: the sleep inherits the ignore through fork and exec.
 *
 * Prints [ OK ] sigexec, or [ FAIL ] sigexec: <case>: <what happened>.
 */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

/* How long the exec may take at most, and how long the sleep gets to die
 * of a signal before it counts as having survived it. */
#define EXEC_WAIT_MS 5000
#define SETTLE_MS 300

static int failed;

static void on_int(int sig) {
    (void)sig;
}

static long now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000L + ts.tv_nsec / 1000000L;
}

/* The name field of /proc/<pid>/status (pid name state ...), "" when the
 * line cannot be read. */
static void proc_name(pid_t pid, char *name, size_t len) {
    char path[64], line[256];
    FILE *f;
    name[0] = 0;
    snprintf(path, sizeof path, "/proc/%d/status", (int)pid);
    f = fopen(path, "r");
    if (!f) {
        return;
    }
    if (fgets(line, sizeof line, f)) {
        char *p = strchr(line, ' ');
        if (p) {
            char *e = strchr(++p, ' ');
            size_t n = e ? (size_t)(e - p) : strlen(p);
            if (n >= len) {
                n = len - 1;
            }
            memcpy(name, p, n);
            name[n] = 0;
        }
    }
    fclose(f);
}

/* True once `pid` runs `sleep` (the exec happened), false when it did not
 * within EXEC_WAIT_MS or ended first. */
static int wait_exec(pid_t pid) {
    long t0 = now_ms();
    char name[64];
    while (now_ms() - t0 < EXEC_WAIT_MS) {
        int st;
        if (waitpid(pid, &st, WNOHANG) == pid) {
            return 0;
        }
        proc_name(pid, name, sizeof name);
        if (strcmp(name, "sleep") == 0) {
            return 1;
        }
        usleep(2000);
    }
    return 0;
}

static void fail(const char *what, const char *why, int st) {
    if (WIFSIGNALED(st)) {
        printf("[ FAIL ] sigexec: %s: %s (killed by signal %d)\n", what, why, WTERMSIG(st));
    } else if (WIFEXITED(st)) {
        printf("[ FAIL ] sigexec: %s: %s (exit status %d)\n", what, why, WEXITSTATUS(st));
    } else {
        printf("[ FAIL ] sigexec: %s: %s\n", what, why);
    }
    failed = 1;
}

/* Fork a child that execs `sleep 30`, with SIGINT set to `child_disp` first
 * (-1: left as inherited), send it SIGINT once it runs sleep, and check
 * that it survives (`survives`) or dies of it. */
static void run_case(const char *what, int child_disp, int survives) {
    pid_t pid;
    int st = 0;
    long t0;
    fflush(stdout);
    pid = fork();
    if (pid < 0) {
        fail(what, "fork failed", 0);
        return;
    }
    if (pid == 0) {
        if (child_disp != -1) {
            struct sigaction sa;
            memset(&sa, 0, sizeof sa);
            sa.sa_handler = child_disp == 0 ? SIG_DFL : SIG_IGN;
            sigemptyset(&sa.sa_mask);
            if (sigaction(SIGINT, &sa, NULL) != 0) {
                _exit(3);
            }
        }
        execlp("sleep", "sleep", "30", (char *)NULL);
        _exit(4);
    }
    if (!wait_exec(pid)) {
        /* Still ours when it exited (status 4: no sleep to exec). */
        if (waitpid(pid, &st, WNOHANG) != pid) {
            kill(pid, SIGKILL);
            waitpid(pid, &st, 0);
            fail(what, "the child did not exec sleep in time", 0);
        } else {
            fail(what, "the child ended before the exec", st);
        }
        return;
    }
    if (kill(pid, SIGINT) != 0) {
        fail(what, "kill(SIGINT) failed", 0);
        kill(pid, SIGKILL);
        waitpid(pid, &st, 0);
        return;
    }
    t0 = now_ms();
    while (now_ms() - t0 < SETTLE_MS) {
        if (waitpid(pid, &st, WNOHANG) == pid) {
            if (survives) {
                fail(what, "the sleep died of the SIGINT it ignores", st);
            } else if (!WIFSIGNALED(st) || WTERMSIG(st) != SIGINT) {
                fail(what, "the sleep ended, but not of SIGINT", st);
            }
            return;
        }
        usleep(5000);
    }
    /* Alive after SETTLE_MS: survived. A SIGTERM then ends it. */
    if (!survives) {
        fail(what, "the sleep survived a SIGINT at its default", 0);
    }
    kill(pid, SIGTERM);
    if (waitpid(pid, &st, 0) != pid || !WIFSIGNALED(st) || WTERMSIG(st) != SIGTERM) {
        fail(what, "the sleep did not end on SIGTERM", st);
    }
}

int main(void) {
    struct sigaction sa, old;
    memset(&sa, 0, sizeof sa);
    sigemptyset(&sa.sa_mask);
    /* What the program itself inherited (a shell's foreground child gets
     * the default): shown for the record, not judged. */
    if (sigaction(SIGINT, NULL, &old) == 0) {
        printf("sigexec: inherited SIGINT %s\n",
               old.sa_handler == SIG_IGN ? "ignored" : old.sa_handler == SIG_DFL ? "at its default" : "caught");
    }
    /* Like a shell: a handler on SIGINT while the jobs are forked. */
    sa.sa_handler = on_int;
    if (sigaction(SIGINT, &sa, NULL) != 0) {
        printf("[ FAIL ] sigexec: sigaction: %s\n", strerror(errno));
        return 1;
    }
    run_case("ign", 1, 1);
    run_case("dfl", -1, 0);
    /* The parent ignores SIGINT: fork and exec keep it ignored. */
    sa.sa_handler = SIG_IGN;
    if (sigaction(SIGINT, &sa, NULL) != 0) {
        printf("[ FAIL ] sigexec: sigaction(SIG_IGN): %s\n", strerror(errno));
        return 1;
    }
    run_case("inh", -1, 1);
    if (failed) {
        return 1;
    }
    printf("[ OK ] sigexec\n");
    return 0;
}
