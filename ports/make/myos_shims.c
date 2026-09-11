/* myos shims for GNU make — tiny gaps libgloss/myos leaves. */
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>

/* libgloss/myos has dup2 and F_DUPFD but no dup(). */
int dup(int oldfd) {
    return fcntl(oldfd, F_DUPFD, 0);
}

/* make's posixos.c references vfork under some guards; plain fork is
 * equivalent for our purposes (no COW tricks expected). */
int vfork(void) {
    return fork();
}

/* HAVE_DECL_GETLOADAVG is 0, but libgloss still lacks the symbol;
 * make only uses it for -l output, which we never enable. */
int getloadavg(double loadavg[], int nelem) {
    (void)loadavg;
    (void)nelem;
    errno = ENOSYS;
    return -1;
}
