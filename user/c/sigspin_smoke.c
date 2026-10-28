/* sigspin-smoke: a signal reaches a process that makes no syscalls.
 *
 * The kernel acts on a task's signals at its syscall exits, and, since
 * issue #647, when an interrupt preempts it in user mode, from the
 * interrupt's frame (kernel/src/signal.rs `on_user_interrupt`); the sender
 * IPIs the CPU the task runs on so that this is at once. Before, a caught
 * signal waited for the next syscall: a `kill` of a program still in its
 * startup, or of a thread spinning on a flag the handler sets, acted only
 * when the program happened to call the kernel, and CI #2315 spent 300 s
 * on a `sleep` whose TERM had arrived a moment too early.
 *
 * A child spins on a flag without a syscall; its parent, once told the
 * child is spinning, sends it the caught signal (`usr1`), or the child's
 * own interval timer does (`alarm`), or a signal at its default ends it
 * (`term`). Each must act within a second; the child checks that values it
 * held in registers across the handler are intact. Prints [ OK ] sigspin.
 */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

/* The most a signal may take to act on the spinning child. */
#define LIMIT_MS 1000
/* The parent's margin for the child to reach its loop once it said so. */
#define SETTLE_MS 20

static volatile sig_atomic_t got;

static void on_sig(int sig) {
    got = sig;
}

static long now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000L + ts.tv_nsec / 1000000L;
}

/* Spin until a handler for `sig` (none when 0) sets `got`, after telling the
 * parent through `ready`; `alarm_ms` arms the interval timer first. Exits
 * 0 when the right signal came and two words kept across the spin are
 * intact, 2 and 3 when not. */
static void spin(int sig, int alarm_ms, int ready) {
    unsigned long pid = (unsigned long)getpid();
    unsigned long a = pid * 7919UL, b = a ^ 0x5bd1e995UL, n = 0;
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = on_sig;
    sigemptyset(&sa.sa_mask);
    if (sig && sigaction(sig, &sa, NULL) != 0) {
        _exit(4);
    }
    if (alarm_ms) {
        struct itimerval it;
        memset(&it, 0, sizeof it);
        it.it_value.tv_usec = alarm_ms * 1000;
        if (setitimer(ITIMER_REAL, &it, NULL) != 0) {
            _exit(5);
        }
    }
    if (write(ready, "r", 1) != 1) {
        _exit(6);
    }
    /* The compiler takes a and b as unknown here, so it keeps and checks
     * them for real; nothing below makes a syscall until the flag is set. */
    __asm__ volatile("" : "+r"(a), "+r"(b));
    while (!got) {
        n++;
    }
    __asm__ volatile("" : "+r"(a), "+r"(b));
    if (a != pid * 7919UL || b != (a ^ 0x5bd1e995UL)) {
        _exit(3);
    }
    _exit(got == sig ? 0 : 2);
}

/* Run one case: a child spinning with a handler for `sig` (0: none) and an
 * interval timer of `alarm_ms` (0: none); `send` is sent once it spins (0:
 * nothing). The child must end within LIMIT_MS of the send (or of its
 * saying it spins), by exit 0 when it has a handler, else by `send`. */
static int run(const char *name, int sig, int alarm_ms, int send) {
    int fds[2], status;
    char c;
    long t0, took;
    pid_t pid;
    if (pipe(fds) != 0) {
        printf("[ FAIL ] sigspin %s: pipe (errno %d)\n", name, errno);
        return 1;
    }
    pid = fork();
    if (pid < 0) {
        printf("[ FAIL ] sigspin %s: fork (errno %d)\n", name, errno);
        return 1;
    }
    if (pid == 0) {
        close(fds[0]);
        spin(sig, alarm_ms, fds[1]);
    }
    close(fds[1]);
    if (read(fds[0], &c, 1) != 1) {
        printf("[ FAIL ] sigspin %s: the child did not say it spins\n", name);
        kill(pid, SIGKILL);
        waitpid(pid, NULL, 0);
        return 1;
    }
    close(fds[0]);
    t0 = now_ms();
    if (send) {
        usleep(SETTLE_MS * 1000);
        t0 = now_ms();
        kill(pid, send);
    }
    if (waitpid(pid, &status, 0) != pid) {
        printf("[ FAIL ] sigspin %s: waitpid (errno %d)\n", name, errno);
        return 1;
    }
    took = now_ms() - t0;
    if (sig && !(WIFEXITED(status) && WEXITSTATUS(status) == 0)) {
        printf("[ FAIL ] sigspin %s: child status %#x after %ld ms (exit 2: another signal, 3: a register lost)\n", name,
               status, took);
        return 1;
    }
    if (!sig && !(WIFSIGNALED(status) && WTERMSIG(status) == send)) {
        printf("[ FAIL ] sigspin %s: child status %#x after %ld ms, not killed by %d\n", name, status, took, send);
        return 1;
    }
    if (took > LIMIT_MS) {
        printf("[ FAIL ] sigspin %s: the signal took %ld ms to act (limit %d)\n", name, took, LIMIT_MS);
        return 1;
    }
    return 0;
}

int main(void) {
    int bad = 0;
    bad |= run("usr1", SIGUSR1, 0, SIGUSR1);
    bad |= run("alarm", SIGALRM, 100, 0);
    bad |= run("term", 0, 0, SIGTERM);
    if (bad) {
        return 1;
    }
    printf("[ OK ] sigspin\n");
    return 0;
}
