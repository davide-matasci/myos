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

/* crt0.S `.globl`s this without a definition or call. TinyCC's linker
 * (pin 2ba12e83) errors on every STB_GLOBAL SHN_UNDEF in the symbol table,
 * even when there is no relocation. GNU ld/lld ignore unused UND, which is
 * why host chello links. Provide a no-op so `tcc -nostdlib -pie` can link
 * crt0+libc+libgloss; this object is already pulled for myos_init_environ.
 */
void myos_libc_init(void) {
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
