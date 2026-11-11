/* The C half of std-mlibc-mix (main.rs): what a C library linked into a
 * Rust program does with the libc they share. */
#include <errno.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* One per thread, at the libc's thread pointer. */
static __thread int calls;

/* A string from malloc, for Rust to read and free. */
char *mix_dup(const char *s) {
    char *p = malloc(strlen(s) + 1);
    if (p != NULL) {
        strcpy(p, s);
    }
    return p;
}

/* Frees what Rust's allocator gave. */
void mix_free(void *p) {
    free(p);
}

/* Sets errno, formats with stdio and counts in the thread-local: 0 when
 * all three are this thread's own (the count starts at 0 in each). */
int mix_thread_state(int value) {
    char buf[32];
    errno = value;
    calls++;
    snprintf(buf, sizeof buf, "%d", errno);
    return atoi(buf) == value && calls == 1 ? 0 : 1;
}

struct call {
    void (*fn)(void *);
    void *arg;
};

static void *run(void *p) {
    struct call *c = p;
    c->fn(c->arg);
    return NULL;
}

/* A thread the C side starts and joins, running Rust's `fn(arg)` on it. */
int mix_on_c_thread(void (*fn)(void *), void *arg) {
    pthread_t t;
    struct call c = {fn, arg};
    if (pthread_create(&t, NULL, run, &c) != 0) {
        return -1;
    }
    return pthread_join(t, NULL);
}

static void (*at_exit_fn)(void);

/* Rust's `fn` at the program's end, through the libc's atexit. */
int mix_atexit(void (*fn)(void)) {
    at_exit_fn = fn;
    return atexit(fn);
}
