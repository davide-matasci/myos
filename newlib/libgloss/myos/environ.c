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
