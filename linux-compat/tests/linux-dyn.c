/*
 * Smoke for dynamically linked Linux binaries under the optional Linux
 * compatibility layer: started by the kernel through its PT_INTERP (musl's
 * /lib/ld-musl-<arch>.so.1), linked against libc.so and libsmoke.so, and
 * dlopen()s libsmoke2.so. Prints "LINUX-DYN OK" when every check passes.
 */
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

extern int smoke_counter;
extern int (*const smoke_ops[])(int);
int smoke_add(int, int);
int smoke_bump(void);
int smoke_tls_next(void);

static int failures;

static void check(int ok, const char *what) {
    printf("%s: %s\n", ok ? "ok" : "FAIL", what);
    if (!ok) {
        failures++;
    }
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    check(smoke_add(2, 3) == 5, "call into libsmoke.so");
    check(smoke_counter == 40 && smoke_bump() == 41 && smoke_counter == 41, "shared data");
    check(smoke_ops[0](21) == 42, "relocated function pointer");
    check(smoke_tls_next() == 8 && smoke_tls_next() == 9, "thread-local in a shared object");

    char buf[64];
    snprintf(buf, sizeof buf, "%d %.2f %s", 42, 1.5, "x");
    check(strcmp(buf, "42 1.50 x") == 0, "printf from libc.so");

    char *p = malloc(300000);
    if (p) {
        memset(p, 1, 300000);
    }
    check(p != NULL && p[299999] == 1, "malloc 300K");
    free(p);

    void *h = dlopen("libsmoke2.so", RTLD_NOW);
    const char *(*name)(void) = h ? (const char *(*)(void))dlsym(h, "smoke2_name") : NULL;
    check(name != NULL && strcmp(name(), "smoke2") == 0, "dlopen/dlsym");
    if (!h) {
        printf("dlerror: %s\n", dlerror());
    }

    if (failures) {
        printf("LINUX-DYN FAIL (%d)\n", failures);
        return 1;
    }
    printf("LINUX-DYN OK\n");
    return 0;
}
