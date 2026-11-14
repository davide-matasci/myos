/* child-smoke: boot-CI guest test for kill(pid, 0) and zombie-free children.
 *
 * kill(pid, 0) finds this process, a running child and this process group,
 * and says ESRCH for a reaped child, as getpgid does. A plain parent keeps its exited child
 * as a zombie for waitpid; one with SA_NOCLDWAIT on SIGCHLD (reported back
 * by sigaction), or ignoring SIGCHLD, gets none: waitpid finds no child
 * (ECHILD), whether asked after the exit or while waiting for it. A parent
 * may move its child to another process group until the child execs, and
 * gets EACCES after. A child's setsid makes it a session leader with no
 * controlling terminal, out of its parent's reach. A child spinning in
 * user mode without syscalls dies of a SIGINT left at its default action
 * (the kernel ends it when it preempts it). A fork shares the parent's
 * pages copy-on-write: the child sees the values from before the fork,
 * and a store by either side, to data, bss, heap, stack or a private
 * mapping, is its own, also two forks deep and after a store the kernel
 * makes (a read into the page). A large program exec'ing a smaller one
 * that is still too large to be reloaded in place leaves the new program
 * an empty heap (the old image's pages past the new span are gone). A
 * write to a pipe whose reader closed ends the writer with SIGPIPE at
 * the default action, and is EPIPE to one that ignores or catches it. A
 * handler's third argument is the interrupted context (ucontext_t): its
 * pc and sp, a uc_sigmask it may change, a pc it may point elsewhere. A
 * caught signal's siginfo_t names its sender (si_pid, si_uid): the process
 * itself, a child that sent it, the child whose exit raised SIGCHLD.
 * Prints [ OK ] child.
 */
#define _GNU_SOURCE /* mlibc: the mcontext_t register names (REG_RIP, gregs) */
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <ucontext.h>
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

static int data_page = 1;
static int bss_page;

/* An image larger than the shell's (3 MiB of bss): exec'ing the shell
 * from it moves the stack down, and the shell's heap starts where this
 * image had its pages, made read-only first like a program's text. */
static char big_image[3 << 20];

/* The shell exec'd from this larger image must get a working heap. */
static int exec_shrink(void) {
    int status;
    uintptr_t lo = ((uintptr_t)big_image + 4095) & ~(uintptr_t)4095;
    uintptr_t hi = ((uintptr_t)big_image + sizeof big_image) & ~(uintptr_t)4095;
    big_image[sizeof big_image - 1] = 1;
    if (mprotect((void *)lo, hi - lo, PROT_READ) != 0) {
        return fail("mprotect of the large image");
    }
    pid_t pid = fork();
    if (pid == 0) {
        execl("/bin/sh", "sh", "-c", "x=$(echo heap) && [ \"$x\" = heap ]", (char *)NULL);
        _exit(127);
    }
    if (pid < 0 || waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        return fail("the shell exec'd from a larger image");
    }
    return 0;
}

/* Every page `kind` a process has: data, bss, heap, stack and a private
 * anonymous mapping, each holding `v` (`set`) or checked for it (`check`). */
static int *heap_page, *stack_page, *map_page;
static int *const *pages[] = {(int *const *)&data_page, (int *const *)&bss_page, &heap_page, &stack_page, &map_page};
static const char *kinds[] = {"data", "bss", "heap", "stack", "mmap"};

static void set_pages(int v) {
    data_page = v;
    bss_page = v;
    *heap_page = v;
    *stack_page = v;
    *map_page = v;
}

/* The first kind that does not hold `v`, or -1. */
static int check_pages(int v) {
    int vals[5] = {data_page, bss_page, *heap_page, *stack_page, *map_page};
    for (int i = 0; i < 5; i++) {
        if (vals[i] != v) {
            return i;
        }
    }
    return -1;
}

/* The child's side of `cow`: the values of before the fork, then its own
 * stores, the parent's not seen even after it stored, and the same for a
 * grandchild. The exit status is the first kind that was wrong + 1, 0
 * when none, 10 + that for the grandchild's view. */
static int cow_child(int go, int v) {
    char c;
    int status, bad;
    if ((bad = check_pages(v)) >= 0) {
        return bad + 1;
    }
    set_pages(v + 1);
    /* The parent stores v + 2 before it writes the byte. */
    (void)!read(go, &c, 1);
    if ((bad = check_pages(v + 1)) >= 0) {
        return bad + 1;
    }
    pid_t pid = fork();
    if (pid == 0) {
        if ((bad = check_pages(v + 1)) >= 0) {
            _exit(10 + bad + 1);
        }
        set_pages(v + 3);
        _exit(check_pages(v + 3) >= 0 ? 20 : 0);
    }
    if (pid < 0 || waitpid(pid, &status, 0) != pid || !WIFEXITED(status)) {
        return 30;
    }
    if (WEXITSTATUS(status) != 0) {
        return WEXITSTATUS(status);
    }
    if ((bad = check_pages(v + 1)) >= 0) {
        return bad + 1;
    }
    /* A store the kernel makes into a shared page (read into it) is this
     * process's own too. */
    int fd[2];
    int saved = *heap_page;
    if (pipe(fd) != 0 || write(fd[1], &v, sizeof v) != sizeof v || read(fd[0], heap_page, sizeof v) != sizeof v) {
        return 40;
    }
    if (*heap_page != v) {
        return 41;
    }
    *heap_page = saved;
    return 0;
}

/* Fork shares pages copy-on-write: each side keeps its own stores. */
static int cow(void) {
    int go[2], status, bad;
    char what[64];
    int stack_slot = 0;
    heap_page = malloc(4096);
    stack_page = &stack_slot;
    map_page = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (heap_page == NULL || map_page == MAP_FAILED || pipe(go) != 0) {
        return fail("cow setup");
    }
    set_pages(100);
    pid_t pid = fork();
    if (pid == 0) {
        close(go[1]);
        _exit(cow_child(go[0], 100));
    }
    close(go[0]);
    if (pid < 0) {
        return fail("cow fork");
    }
    set_pages(102);
    (void)!write(go[1], "g", 1);
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status)) {
        return fail("cow child");
    }
    if (WEXITSTATUS(status) != 0) {
        int code = WEXITSTATUS(status);
        const char *kind = code >= 1 && code <= 5 ? kinds[code - 1] : code >= 11 && code <= 15 ? kinds[code - 11] : "?";
        snprintf(what, sizeof what, "cow: the child's view of %s (code %d)", kind, code);
        return fail(what);
    }
    if ((bad = check_pages(102)) >= 0) {
        snprintf(what, sizeof what, "cow: the child's store to %s reached the parent", kinds[bad]);
        return fail(what);
    }
    close(go[1]);
    free(heap_page);
    munmap(map_page, 4096);
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

/* More processes alive at once than the old limit of 64 tasks in the
 * whole system: each waits for the parent to close the pipe. */
static int many_children(void) {
    enum { N = 80 };
    pid_t kids[N];
    int p[2], made = 0, bad = 0, status;
    char c;
    if (pipe(p) != 0) {
        return fail("pipe for many children");
    }
    while (made < N) {
        pid_t pid = fork();
        if (pid == 0) {
            close(p[1]);
            _exit(read(p[0], &c, 1) == 0 ? 0 : 1);
        }
        if (pid < 0) {
            break;
        }
        kids[made++] = pid;
    }
    close(p[0]);
    close(p[1]);
    for (int i = 0; i < made; i++) {
        bad |= waitpid(kids[i], &status, 0) != kids[i] || !WIFEXITED(status) || WEXITSTATUS(status) != 0;
    }
    if (made < N) {
        return fail("80 children at once");
    }
    return bad ? fail("80 children's exits") : 0;
}

static volatile sig_atomic_t pipe_signals;

static void on_pipe(int sig) {
    (void)sig;
    pipe_signals++;
}

/* A pipe whose reader closed: the writer dies of SIGPIPE at the default
 * action (a forked child, so this process survives), gets EPIPE with the
 * signal ignored, and the handler plus EPIPE with one caught. */
static int sigpipe(void) {
    int p[2];
    int status;
    pid_t pid;
    char ch = 'x';
    if (pipe(p) != 0 || close(p[0]) != 0) {
        return fail("sigpipe: pipe");
    }
    pid = fork();
    if (pid == 0) {
        write(p[1], &ch, 1);
        _exit(3); /* survived: not killed */
    }
    if (pid < 0 || waitpid(pid, &status, 0) != pid) {
        return fail("sigpipe: fork");
    }
    if (!WIFSIGNALED(status) || WTERMSIG(status) != SIGPIPE) {
        printf("[ FAIL ] child sigpipe: the writer's status is 0x%x, not death by SIGPIPE\n", status);
        return 1;
    }
    if (signal(SIGPIPE, SIG_IGN) == SIG_ERR) {
        return fail("sigpipe: SIG_IGN");
    }
    errno = 0;
    if (write(p[1], &ch, 1) != -1 || errno != EPIPE) {
        return fail("sigpipe: ignored, the write is not EPIPE");
    }
    if (signal(SIGPIPE, on_pipe) == SIG_ERR) {
        return fail("sigpipe: handler");
    }
    errno = 0;
    if (write(p[1], &ch, 1) != -1 || errno != EPIPE || pipe_signals != 1) {
        return fail("sigpipe: caught, the write is not EPIPE with the handler run once");
    }
    signal(SIGPIPE, SIG_DFL);
    close(p[1]);
    return 0;
}

static volatile sig_atomic_t from_pid, from_uid;

static void on_info(int sig, siginfo_t *si, void *uc) {
    (void)sig;
    (void)uc;
    from_pid = si->si_pid;
    from_uid = si->si_uid;
}

static int catch_info(int sig) {
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = on_info;
    sa.sa_flags = SA_SIGINFO;
    sigemptyset(&sa.sa_mask);
    return sigaction(sig, &sa, NULL);
}

/* The sender in a caught signal's siginfo_t: this process for its own
 * kill (and its uid), the child for one the child sends, and for the
 * SIGCHLD of the child's exit. */
static int sender(void) {
    pid_t me = getpid();
    pid_t pid;
    int status;
    if (catch_info(SIGUSR1) != 0 || catch_info(SIGCHLD) != 0) {
        return fail("sender: sigaction");
    }
    from_pid = -1;
    kill(me, SIGUSR1);
    if (from_pid != me || from_uid != (int)getuid()) {
        printf("[ FAIL ] child sender: own kill from pid %d uid %d, not %d %d\n", (int)from_pid, (int)from_uid,
               (int)me, (int)getuid());
        return 1;
    }
    from_pid = -1;
    pid = fork();
    if (pid == 0) {
        kill(me, SIGUSR1);
        for (;;) {
            sleep(1); /* until the parent's SIGTERM */
        }
    }
    for (int i = 0; i < 300 && from_pid == -1; i++) {
        usleep(10 * 1000);
    }
    if (from_pid != pid) {
        printf("[ FAIL ] child sender: the child's kill from pid %d, not %d\n", (int)from_pid, (int)pid);
        return 1;
    }
    from_pid = -1;
    kill(pid, SIGTERM);
    while (waitpid(pid, &status, 0) == -1 && errno == EINTR) {
    }
    if (from_pid != pid) {
        printf("[ FAIL ] child sender: SIGCHLD from pid %d, not %d\n", (int)from_pid, (int)pid);
        return 1;
    }
    signal(SIGUSR1, SIG_DFL);
    signal(SIGCHLD, SIG_DFL);
    return 0;
}

/* The interrupted pc and sp of a handler's ucontext_t. */
#if defined(__x86_64__)
#define UC_PC(uc) ((uc)->uc_mcontext.gregs[REG_RIP])
#define UC_SP(uc) ((uc)->uc_mcontext.gregs[REG_RSP])
#elif defined(__aarch64__)
#define UC_PC(uc) ((uc)->uc_mcontext.pc)
#define UC_SP(uc) ((uc)->uc_mcontext.sp)
#else
#define UC_PC(uc) ((uc)->uc_mcontext.__gregs[REG_PC])
#define UC_SP(uc) ((uc)->uc_mcontext.__gregs[REG_SP])
#endif

static volatile uintptr_t ctx_pc, ctx_sp;
static volatile sig_atomic_t ctx_calls, ctx_landed;
static int ctx_pipe[2];

/* SIGUSR1: what the context says was interrupted; SIGUSR2 comes back
 * blocked from it. */
static void on_context(int sig, siginfo_t *si, void *arg) {
    ucontext_t *uc = arg;
    (void)sig;
    (void)si;
    ctx_calls++;
    if (uc != NULL) {
        ctx_pc = (uintptr_t)UC_PC(uc);
        ctx_sp = (uintptr_t)UC_SP(uc);
        sigaddset(&uc->uc_sigmask, SIGUSR2);
    }
}

/* Where on_fault sends the faulting thread: it never returns there. */
static void landed(void) {
    ctx_landed = 1;
    /* The parent reads the byte: this child got here, by the pc alone. */
    write(ctx_pipe[1], "L", 1);
    _exit(9);
}

static void on_fault(int sig, siginfo_t *si, void *arg) {
    ucontext_t *uc = arg;
    (void)sig;
    (void)si;
    if (uc == NULL) {
        _exit(3);
    }
    UC_PC(uc) = (uintptr_t)landed;
#if defined(__x86_64__)
    /* As after a call: the ABI's alignment at a function's entry. */
    UC_SP(uc) = (UC_SP(uc) & ~(uintptr_t)15) - 8;
#endif
}

/* A handler's third argument is the interrupted context: its sp is on
 * this thread's stack and its pc in this program's text; a signal added
 * to its uc_sigmask is blocked once the handler returned; and a fault's
 * handler that points its pc at another function resumes there (a forked
 * child, which says so through a pipe and exits). */
static int context(void) {
    struct sigaction sa;
    sigset_t now;
    char here, byte = 0;
    int status;
    pid_t pid;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = on_context;
    sa.sa_flags = SA_SIGINFO;
    if (sigaction(SIGUSR1, &sa, NULL) != 0 || raise(SIGUSR1) != 0 || ctx_calls != 1) {
        return fail("context: the handler did not run");
    }
    if (ctx_pc == 0 || ctx_sp == 0) {
        return fail("context: no ucontext, or an empty one");
    }
    uintptr_t at = (uintptr_t)&here;
    if (ctx_sp > at + 65536 || at > ctx_sp + 65536) {
        printf("[ FAIL ] child context: sp %#lx is not near this frame (%#lx)\n", (unsigned long)ctx_sp,
               (unsigned long)at);
        return 1;
    }
    if (sigprocmask(SIG_BLOCK, NULL, &now) != 0 || !sigismember(&now, SIGUSR2) || sigismember(&now, SIGUSR1)) {
        return fail("context: uc_sigmask's SIGUSR2 is not blocked after the handler");
    }
    sigemptyset(&now);
    sigaddset(&now, SIGUSR2);
    sigprocmask(SIG_UNBLOCK, &now, NULL);
    signal(SIGUSR1, SIG_DFL);

    if (pipe(ctx_pipe) != 0) {
        return fail("context: pipe");
    }
    pid = fork();
    if (pid == 0) {
        sa.sa_sigaction = on_fault;
        sigaction(SIGSEGV, &sa, NULL);
        sigaction(SIGBUS, &sa, NULL);
        *(volatile int *)(uintptr_t)8 = 1;
        _exit(4); /* resumed after the store: the pc was not taken */
    }
    close(ctx_pipe[1]);
    if (pid < 0 || waitpid(pid, &status, 0) != pid) {
        return fail("context: fork");
    }
    if (read(ctx_pipe[0], &byte, 1) != 1 || byte != 'L' || !WIFEXITED(status) || WEXITSTATUS(status) != 9) {
        printf("[ FAIL ] child context: a handler's pc was not resumed at (status %#x)\n", status);
        return 1;
    }
    close(ctx_pipe[0]);
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
    /* Gone for getpgid too, however briefly its slot outlives the reap. */
    errno = 0;
    if (getpgid(pid) != -1 || errno != ESRCH) {
        return fail("getpgid(reaped child) is not ESRCH");
    }

    /* A plain parent: the exited child waits as a zombie. */
    pid = child(0);
    usleep(300 * 1000);
    if (pid < 0 || waitpid(pid, &status, WNOHANG) != pid || WEXITSTATUS(status) != 7) {
        return fail("a plain parent's zombie");
    }

    if (spinning_child() != 0 || many_children() != 0) {
        return 1;
    }
    if (setsid_child() != 0) {
        return 1;
    }
    if (setpgid_exec() != 0) {
        return 1;
    }
    if (cow() != 0) {
        return 1;
    }
    if (exec_shrink() != 0) {
        return 1;
    }
    if (sigpipe() != 0) {
        return 1;
    }
    if (context() != 0) {
        return 1;
    }
    if (sender() != 0) {
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
