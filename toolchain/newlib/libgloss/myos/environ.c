#include <stddef.h>

extern char **environ;

void myos_init_environ(int argc, char **argv) {
    (void)argc;
    char **p = argv;
    while (*p) {
        p++;
    }
    environ = p + 1;
}

/* GNU clearenv(3). newlib libc has getenv/setenv but not this.
 * Empty env is enough for fake-root login (which setenv()s HOME/SHELL/USER
 * afterwards). Point at a static {NULL} rather than NULL so newlib setenv
 * can walk environ without crashing.
 */
int clearenv(void) {
    static char *empty[] = { NULL };
    environ = empty;
    return 0;
}
