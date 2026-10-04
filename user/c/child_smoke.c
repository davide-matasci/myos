/* child-smoke: boot-CI guest test for kill(pid, 0) and zombie-free children.
 *
 * kill(pid, 0) finds this process, a running child and this process group,
 * and says ESRCH for a reaped child. A plain parent keeps its exited child
 * as a zombie for waitpid; one with SA_NOCLDWAIT on SIGCHLD (reported back
 * by sigaction), or ignoring SIGCHLD, gets none: waitpid finds no child
 * (ECHILD), whether asked after the exit or while waiting for it. Prints
 * [ OK ] child.
 */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static int fail(const char *what) {
    printf("[ FAIL ] child %s (errno %d)\n", what, errno);
    return 1;
}

/* A child that sleeps `ms`, then exits with status 7. */
static pid_t child(int ms) {
    pid_t pid = fork();
    if (pid == 0) {
        usleep(ms * 1000);
        _exit(7);
    }
    return pid;
}

static int set_sigchld(void (*handler)(int), int flags) {
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = handler;
    sa.sa_flags = flags;
    sigemptyset(&sa.sa_mask);
    return sigaction(SIGCHLD, &sa, NULL);
}

/* With the current SIGCHLD disposition: no zombie after a child's exit, and
 * a wait that outlives the last child ends with ECHILD. */
static int no_zombies(const char *how) {
    char what[64];
    int status;
    pid_t pid = child(0);
    usleep(300 * 1000);
    errno = 0;
    if (pid < 0 || waitpid(pid, &status, WNOHANG) != -1 || errno != ECHILD) {
        snprintf(what, sizeof what, "%s: a zombie was left", how);
        return fail(what);
    }
    child(200);
    errno = 0;
    if (waitpid(-1, &status, 0) != -1 || errno != ECHILD) {
        snprintf(what, sizeof what, "%s: wait did not end with ECHILD", how);
        return fail(what);
    }
    return 0;
}

int main(void) {
    struct sigaction old;
    int status;

    /* kill(pid, 0): only whether the target exists. */
    if (kill(getpid(), 0) != 0) {
        return fail("kill(self, 0)");
    }
    if (kill(-getpgrp(), 0) != 0) {
        return fail("kill(-pgrp, 0)");
    }
    pid_t pid = child(300);
    if (pid < 0 || kill(pid, 0) != 0) {
        return fail("kill(running child, 0)");
    }
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 7) {
        return fail("waitpid");
    }
    errno = 0;
    if (kill(pid, 0) != -1 || errno != ESRCH) {
        return fail("kill(reaped child, 0) is not ESRCH");
    }

    /* A plain parent: the exited child waits as a zombie. */
    pid = child(0);
    usleep(300 * 1000);
    if (pid < 0 || waitpid(pid, &status, WNOHANG) != pid || WEXITSTATUS(status) != 7) {
        return fail("a plain parent's zombie");
    }

    /* SA_NOCLDWAIT (with the default action), then SIGCHLD ignored. */
    if (set_sigchld(SIG_DFL, SA_NOCLDWAIT) != 0) {
        return fail("sigaction SA_NOCLDWAIT");
    }
    if (sigaction(SIGCHLD, NULL, &old) != 0 || !(old.sa_flags & SA_NOCLDWAIT)) {
        return fail("sigaction does not report SA_NOCLDWAIT");
    }
    if (no_zombies("SA_NOCLDWAIT")) {
        return 1;
    }
    if (set_sigchld(SIG_IGN, 0) != 0 || no_zombies("SIGCHLD ignored")) {
        return 1;
    }
    printf("[ OK ] child\n");
    return 0;
}
