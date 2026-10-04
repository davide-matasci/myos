/* Runtime stubs for Lynx on myos — only what differs from libgloss. */
#include <errno.h>

/* system(3) — refuse external commands (lynx would shell out for
 * downloads and editors). */
int system(const char *cmd) {
    (void)cmd;
    errno = ENOSYS;
    return -1;
}
