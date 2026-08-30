#include <stddef.h>
#include <string.h>

char *environ;

char *
getenv(const char *name)
{
    char **p;
    size_t n;

    if (name == NULL || environ == NULL)
        return NULL;
    n = strlen(name);
    for (p = environ; *p != NULL; p++) {
        if (strncmp(*p, name, n) == 0 && (*p)[n] == '=')
            return *p + n + 1;
    }
    return NULL;
}

void myos_init_environ(int argc, char **argv) {
    (void)argc;
    char **p = argv;
    while (*p) {
        p++;
    }
    environ = (char *)(p + 1);
}
