/* myos shims for GNU make — the one gap libgloss/myos leaves. */
#include <errno.h>

/* HAVE_DECL_GETLOADAVG is 0, but libgloss still lacks the symbol;
 * make only uses it for -l output, which we never enable. */
int getloadavg(double loadavg[], int nelem) {
    (void)loadavg;
    (void)nelem;
    errno = ENOSYS;
    return -1;
}
