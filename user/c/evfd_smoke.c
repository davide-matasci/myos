/* evfd-smoke: boot-CI guest test for the event counters (/dev/event) and
 * timers (/dev/timer) behind eventfd(2) and timerfd(2) (kernel/src/evfd.rs,
 * libgloss evfd.c, docs/events.md).
 *
 * An event counter adds up its writes and a read takes the whole count;
 * poll says POLLOUT while it is empty, POLLIN too once it is not; an empty
 * nonblocking read is EAGAIN, a short read or a write of ~0 refused; the
 * semaphore mode takes one per read, from the initial value; a blocked read
 * waits for a forked child's write (not before). /proc/self/fd names the
 * fd's directory, whose status shows the count. A one-shot timer expires
 * once, after its delay (a read waits for it, poll says POLLIN then, not
 * before); a periodic one counts its expirations and gettime shows what is
 * left; disarming stops it; an absolute CLOCK_MONOTONIC value works. Prints
 * [ OK ] evfd.
 */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <string.h>
#include <sys/eventfd.h>
#include <sys/stat.h>
#include <sys/timerfd.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define DELAY_MS 300

static int fail(const char *what) {
    printf("[ FAIL ] evfd %s (errno %d)\n", what, errno);
    return 1;
}

static long now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000L + ts.tv_nsec / 1000000L;
}

static int poll_bits(int fd, int timeout_ms) {
    struct pollfd p = {fd, POLLIN | POLLOUT, 0};
    int n = poll(&p, 1, timeout_ms);
    return n < 0 ? -1 : n == 0 ? 0 : p.revents;
}

/* The `status` of the object behind `fd`, by its /proc/self/fd link. */
static int read_status(int fd, char *buf, size_t cap) {
    char link[32], path[64];
    ssize_t n;
    int sfd;
    snprintf(link, sizeof link, "/proc/self/fd/%d", fd);
    n = readlink(link, path, sizeof path - 8);
    if (n <= 5 || strcmp(path + n - 5, "/data") != 0) {
        return -1;
    }
    strcpy(path + n - 5, "/status");
    sfd = open(path, O_RDONLY);
    if (sfd < 0) {
        return -1;
    }
    n = read(sfd, buf, cap - 1);
    close(sfd);
    if (n < 0) {
        return -1;
    }
    buf[n] = '\0';
    return 0;
}

static int events(void) {
    eventfd_t v = 0;
    char status[64];
    pid_t pid;
    long t0;
    int status_code = 0;
    int fd = eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC);
    if (fd < 0) {
        return fail("eventfd");
    }
    if (poll_bits(fd, 0) != POLLOUT) {
        return fail("an empty counter is only writable");
    }
    if (read(fd, &v, sizeof v) != -1 || errno != EAGAIN) {
        return fail("an empty nonblocking read is EAGAIN");
    }
    if (eventfd_write(fd, 3) != 0 || eventfd_write(fd, 4) != 0) {
        return fail("write");
    }
    if (read_status(fd, status, sizeof status) != 0 || strncmp(status, "count 7\n", 8) != 0) {
        return fail("status shows the count");
    }
    if (poll_bits(fd, 0) != (POLLIN | POLLOUT)) {
        return fail("a counter with a count is readable");
    }
    if (eventfd_read(fd, &v) != 0 || v != 7) {
        return fail("a read takes the whole count");
    }
    if (poll_bits(fd, 0) != POLLOUT) {
        return fail("empty again after the read");
    }
    /* Refused (EINVAL on Linux; the native ABI has no EINVAL result, so
     * libgloss says EBADF). */
    v = ~(eventfd_t)0;
    if (write(fd, &v, sizeof v) != -1) {
        return fail("a write of ~0 is refused");
    }
    if (eventfd_write(fd, 1) != 0 || read(fd, &v, 4) != -1) {
        return fail("a short read is refused");
    }
    close(fd);

    fd = eventfd(3, EFD_SEMAPHORE);
    if (fd < 0) {
        return fail("eventfd semaphore");
    }
    if (eventfd_read(fd, &v) != 0 || v != 1 || eventfd_read(fd, &v) != 0 || v != 1
        || eventfd_read(fd, &v) != 0 || v != 1) {
        return fail("a semaphore read takes one");
    }
    if (poll_bits(fd, 0) != POLLOUT) {
        return fail("the semaphore is empty after three");
    }
    /* A read waits for a child's write: the counter is the same object in
     * both processes. */
    t0 = now_ms();
    pid = fork();
    if (pid == 0) {
        usleep(DELAY_MS * 1000);
        _exit(eventfd_write(fd, 2) == 0 ? 0 : 1);
    }
    if (eventfd_read(fd, &v) != 0 || v != 1) {
        return fail("a blocked read gets the child's write");
    }
    if (now_ms() - t0 < DELAY_MS / 2) {
        return fail("the read returned before the write");
    }
    if (waitpid(pid, &status_code, 0) != pid || status_code != 0) {
        return fail("child");
    }
    if (eventfd_read(fd, &v) != 0 || v != 1 || poll_bits(fd, 0) != POLLOUT) {
        return fail("the second unit of the child's write");
    }
    close(fd);
    return 0;
}

static int timers(void) {
    struct itimerspec its = {{0, 0}, {0, 0}}, cur;
    struct timespec now;
    unsigned long long n = 0;
    long t0;
    int fd = timerfd_create(CLOCK_MONOTONIC, TFD_CLOEXEC);
    if (fd < 0) {
        return fail("timerfd_create");
    }
    if (timerfd_gettime(fd, &cur) != 0 || cur.it_value.tv_sec != 0 || cur.it_value.tv_nsec != 0) {
        return fail("a new timer is disarmed");
    }
    if (poll_bits(fd, 0) != 0) {
        return fail("a disarmed timer is not readable");
    }
    /* One shot, 100 ms ahead. */
    its.it_value.tv_nsec = 100 * 1000000L;
    t0 = now_ms();
    if (timerfd_settime(fd, 0, &its, NULL) != 0) {
        return fail("settime one-shot");
    }
    if (timerfd_gettime(fd, &cur) != 0 || cur.it_value.tv_sec != 0 || cur.it_value.tv_nsec == 0
        || cur.it_value.tv_nsec > 100 * 1000000L || cur.it_interval.tv_nsec != 0) {
        return fail("gettime of an armed one-shot");
    }
    if (poll_bits(fd, 0) != 0) {
        return fail("not expired yet");
    }
    if (poll_bits(fd, 5000) != POLLIN) {
        return fail("poll waits for the expiry");
    }
    if (now_ms() - t0 < 50) {
        return fail("expired too early");
    }
    if (read(fd, &n, sizeof n) != (ssize_t)sizeof n || n != 1) {
        return fail("one expiration");
    }
    if (poll_bits(fd, 0) != 0 || timerfd_gettime(fd, &cur) != 0 || cur.it_value.tv_nsec != 0) {
        return fail("a one-shot is disarmed after its expiry");
    }
    /* Periodic: 20 ms, first in 20 ms; about five in 100 ms. */
    its.it_value.tv_nsec = 20 * 1000000L;
    its.it_interval.tv_nsec = 20 * 1000000L;
    if (timerfd_settime(fd, 0, &its, NULL) != 0) {
        return fail("settime periodic");
    }
    usleep(100 * 1000);
    if (read(fd, &n, sizeof n) != (ssize_t)sizeof n || n < 3 || n > 15) {
        return fail("periodic expirations");
    }
    if (timerfd_gettime(fd, &cur) != 0 || cur.it_interval.tv_nsec != 20 * 1000000L
        || cur.it_value.tv_nsec == 0 || cur.it_value.tv_nsec > 20 * 1000000L) {
        return fail("gettime of a periodic timer");
    }
    /* A read waits for the next expiry (one or two of them). */
    t0 = now_ms();
    if (read(fd, &n, sizeof n) != (ssize_t)sizeof n || n == 0 || n > 2 || now_ms() - t0 > 1000) {
        return fail("a read waits for the next expiry");
    }
    /* Disarm: settime returns what was left; nothing more expires. */
    memset(&its, 0, sizeof its);
    if (timerfd_settime(fd, 0, &its, &cur) != 0 || cur.it_interval.tv_nsec != 20 * 1000000L) {
        return fail("disarm returns the old setting");
    }
    usleep(50 * 1000);
    if (poll_bits(fd, 0) != 0 || timerfd_gettime(fd, &cur) != 0 || cur.it_value.tv_nsec != 0
        || cur.it_interval.tv_nsec != 0) {
        return fail("disarmed");
    }
    /* An absolute time on CLOCK_MONOTONIC, 100 ms ahead. */
    clock_gettime(CLOCK_MONOTONIC, &now);
    its.it_value.tv_sec = now.tv_sec;
    its.it_value.tv_nsec = now.tv_nsec + 100 * 1000000L;
    if (its.it_value.tv_nsec >= 1000000000L) {
        its.it_value.tv_sec++;
        its.it_value.tv_nsec -= 1000000000L;
    }
    t0 = now_ms();
    if (timerfd_settime(fd, TFD_TIMER_ABSTIME, &its, NULL) != 0) {
        return fail("settime absolute");
    }
    if (read(fd, &n, sizeof n) != (ssize_t)sizeof n || n != 1 || now_ms() - t0 < 50) {
        return fail("an absolute one-shot");
    }
    /* Bad arguments. */
    its.it_value.tv_nsec = 1000000000L;
    if (timerfd_settime(fd, 0, &its, NULL) != -1 || errno != EINVAL) {
        return fail("a nanosecond field of a second is EINVAL");
    }
    if (timerfd_create(99, 0) != -1 || errno != EINVAL) {
        return fail("an unknown clock is EINVAL");
    }
    if (timerfd_gettime(0, &cur) != -1) {
        return fail("gettime on a terminal fails");
    }
    close(fd);
    return 0;
}

int main(void) {
    if (events() != 0 || timers() != 0) {
        return 1;
    }
    printf("[ OK ] evfd\n");
    return 0;
}
