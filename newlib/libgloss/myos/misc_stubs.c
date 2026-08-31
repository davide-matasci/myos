#include <errno.h>
#include <signal.h>
#include <unistd.h>

int sigaction(int sig, const struct sigaction *restrict act,
    struct sigaction *restrict oact) {
    (void)sig;
    (void)act;
    (void)oact;
    errno = ENOSYS;
    return -1;
}

void sync(void) {
}

char *ttyname(int fd) {
    (void)fd;
    errno = ENOTTY;
    return NULL;
}
