/*
 * A slave write to a pty whose master is open but never read fills the
 * output ring and blocks; the blocked write must be interruptible by a
 * signal, not a 100%-CPU unkillable kernel spin (kernel/src/pty.rs
 * `slave_write` once read the wait sequence before its own `notify`, so it
 * busy-returned from `block_until` forever and never checked for signals —
 * not even SIGKILL could stop it).
 *
 * The child fills the ring and blocks writing; the parent SIGKILLs it and
 * reaps it. If the write were unkillable the reap would hang and the boot
 * would time out. Prints one `[ OK ] fault` / `[ FAIL ] fault ...` line.
 *
 * (The containment of CPU faults — an illegal instruction, divide-by-zero —
 * is not exercised here: the boot-test host treats the `[ WARN ] user fault`
 * line the kernel prints when it kills a faulting task as a failure, so a
 * test cannot fault on purpose. That path is checked out of band.)
 */
#define _GNU_SOURCE 1
#include <errno.h>
#include <pty.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <sys/wait.h>
#include <unistd.h>

int main(void) {
    int m, s;
    char name[64];
    if (openpty(&m, &s, name, NULL, NULL) != 0) {
        printf("[ FAIL ] fault: openpty (%s)\n", strerror(errno));
        return 1;
    }
    pid_t p = fork();
    if (p == 0) {
        // Master stays open (in the parent and here) but nobody reads it, so
        // the ring fills after OUT_CAP bytes and this write blocks.
        static char buf[65536];
        memset(buf, 'x', sizeof buf);
        while (write(s, buf, sizeof buf) > 0) {
        }
        _exit(0);
    }
    // Give the child time to fill the ring and block in the kernel.
    struct timespec ts = {0, 400 * 1000 * 1000};
    nanosleep(&ts, NULL);
    // A blocked slave writer must be killable.
    kill(p, SIGKILL);
    int st = 0;
    (void)waitpid(p, &st, 0);
    close(m);
    close(s);
    if (!WIFSIGNALED(st) || WTERMSIG(st) != SIGKILL) {
        printf("[ FAIL ] fault: blocked pty writer not killed by SIGKILL (st=%d)\n", st);
        return 1;
    }
    printf("[ OK ] fault\n");
    return 0;
}
