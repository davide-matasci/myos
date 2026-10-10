/* child-smoke: boot-CI guest test for kill(pid, 0) and zombie-free children.
 *
 * kill(pid, 0) finds this process, a running child and this process group,
 * and says ESRCH for a reaped child. A plain parent keeps its exited child
 * as a zombie for waitpid; one with SA_NOCLDWAIT on SIGCHLD (reported back
 * by sigaction), or ignoring SIGCHLD, gets none: waitpid finds no child
 * (ECHILD), whether asked after the exit or while waiting for it. A parent
 * may move its child to another process group until the child execs, and
 * gets EACCES after. A child's setsid makes it a session leader with no
 * controlling terminal, out of its parent's reach. A child spinning in
 * user mode without syscalls dies of a SIGINT left at its default action
 * (the kernel ends it when it preempts it). Prints [ OK ] child.
 */
#include <errno.h>
#include <fcntl.h>
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

/* setpgid on a child: allowed before its exec, EACCES after. */
static int setpgid_exec(void) {
    int go[2], execd[2], status;
    char c;
    if (pipe(go) != 0 || pipe(execd) != 0 || fcntl(execd[1], F_SETFD, FD_CLOEXEC) != 0) {
        return fail("pipes");
    }
    pid_t pid = fork();
    if (pid == 0) {
        close(go[1]);
        close(execd[0]);
        (void)!read(go[0], &c, 1);
        execl("/bin/sbase/sleep", "sleep", "10", (char *)NULL);
        _exit(127);
    }
    close(go[0]);
    close(execd[1]);
    if (pid < 0 || setpgid(pid, pid) != 0) {
        return fail("setpgid on a child before its exec");
    }
    (void)!write(go[1], "g", 1);
    /* The close-on-exec end closes at the exec: EOF. */
    if (read(execd[0], &c, 1) != 0) {
        return fail("the child's exec");
    }
    errno = 0;
    if (setpgid(pid, getpgrp()) != -1 || errno != EACCES) {
        return fail("setpgid on a child after its exec: not EACCES");
    }
    kill(pid, SIGKILL);
    waitpid(pid, &status, 0);
    close(go[1]);
    close(execd[0]);
    return 0;
}

/* A forked child starts a session: it leads it and a group of the same
 * id, has no controlling terminal, cannot start another or change its group,
 * and its parent, now in another session, cannot move it either. Its own
 * child inherits the session. */
static int setsid_child(void) {
    int go[2], status;
    char c;
    if (pipe(go) != 0) {
        return fail("pipe");
    }
    pid_t pid = fork();
    if (pid == 0) {
        close(go[0]);
        pid_t me = getpid();
        if (setsid() != me || getsid(0) != me || getpgrp() != me) {
            _exit(1);
        }
        if (setsid() != -1 || errno != EPERM || setpgid(0, 0) != -1 || errno != EPERM) {
            _exit(2);
        }
        if (open("/dev/tty", O_RDWR) >= 0) {
            _exit(3);
        }
        pid_t g = fork();
        if (g == 0) {
            _exit(getsid(0) == me && getsid(getppid()) == me ? 0 : 1);
        }
        if (g < 0 || waitpid(g, &status, 0) != g || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            _exit(4);
        }
        (void)!write(go[1], "s", 1);
        sleep(10);
        _exit(0);
    }
    close(go[1]);
    if (pid < 0 || read(go[0], &c, 1) != 1) {
        waitpid(pid, &status, 0);
        printf("[ FAIL ] child setsid: step %d\n", WIFEXITED(status) ? WEXITSTATUS(status) : -1);
        return 1;
    }
    errno = 0;
    if (getsid(pid) != pid || getsid(0) == pid || setpgid(pid, getpgrp()) != -1 || errno != EPERM) {
        return fail("setsid: the parent's view");
    }
    kill(pid, SIGKILL);
    waitpid(pid, &status, 0);
    close(go[0]);
    return 0;
}

/* A child that never makes a syscall (a pure CPU loop, as an interrupted
 * computation): SIGINT at its default action still ends it. */
static int spinning_child(void) {
    int status, i;
    pid_t pid = fork();
    if (pid == 0) {
        for (;;) {
            __asm__ volatile("" ::: "memory");
        }
    }
    if (pid < 0) {
        return fail("fork a spinning child");
    }
    usleep(200 * 1000);
    if (kill(pid, SIGINT) != 0) {
        return fail("kill(spinning child, SIGINT)");
    }
    for (i = 0; i < 50 && waitpid(pid, &status, WNOHANG) == 0; i++) {
        usleep(100 * 1000);
    }
    if (i == 50) {
        kill(pid, SIGKILL);
        waitpid(pid, &status, 0);
        return fail("SIGINT left a spinning child running");
    }
    if (!WIFSIGNALED(status) || WTERMSIG(status) != SIGINT) {
        return fail("a spinning child's SIGINT status");
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

    if (spinning_child() != 0) {
        return 1;
    }
    if (setsid_child() != 0) {
        return 1;
    }
    if (setpgid_exec() != 0) {
        return 1;
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
