/* Runtime stubs for Lynx on myos — only symbols not already in libgloss. */
#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <unistd.h>

int dup(int oldfd) {
    return fcntl(oldfd, F_DUPFD, 0);
}

/* system(3) — refuse external commands. */
int system(const char *cmd) {
    (void)cmd;
    errno = ENOSYS;
    return -1;
}
