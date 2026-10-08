/* Constructors and destructors (crt0 runs newlib's __libc_init_array before
 * main and registers __libc_fini_array with atexit): the constructors run
 * before main, by priority, with environ set up; the destructors run at
 * exit, after the atexit handlers main registered, with stdio still there.
 * main returns 3, which the boot test takes for a failure; the last
 * destructor turns it into 0 with _exit when everything ran in order, so a
 * destructor that never runs fails the test too. Prints one `[ OK ] ctor`
 * or a `[ FAIL ] ctor ...` line. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

extern char **environ;

static char order[8];
static int n;
static int environ_set;

static void note(char c) {
    if (n < (int)sizeof order - 1) {
        order[n++] = c;
    }
}

__attribute__((constructor(102))) static void second(void) {
    note('b');
}

__attribute__((constructor(101))) static void first(void) {
    note('a');
    environ_set = environ != NULL;
}

__attribute__((constructor)) static void last(void) {
    note('c');
}

static void at_exit(void) {
    note('x');
}

/* Destructors run in the reverse order of their priorities: `done` (101)
 * after `closing` (102). */
__attribute__((destructor(102))) static void closing(void) {
    note('y');
}

__attribute__((destructor(101))) static void done(void) {
    note('z');
    if (strcmp(order, "abcmxyz") != 0) {
        printf("[ FAIL ] ctor order %s (want abcmxyz)\n", order);
        return;
    }
    printf("[ OK ] ctor\n");
    fflush(stdout);
    _exit(0);
}

int main(void) {
    if (!environ_set) {
        printf("[ FAIL ] ctor environ not set up before the constructors\n");
        return 1;
    }
    note('m');
    if (atexit(at_exit) != 0) {
        printf("[ FAIL ] ctor atexit\n");
        return 1;
    }
    return 3;
}
