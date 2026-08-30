#include <stddef.h>

char *environ;

void myos_init_environ(int argc, char **argv) {
    (void)argc;
    char **p = argv;
    while (*p) {
        p++;
    }
    environ = (char *)(p + 1);
}
