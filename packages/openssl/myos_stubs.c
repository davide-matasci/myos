/* Runtime stubs for openssl(1) on myos: only what libgloss has no honest
 * implementation of. */
#include <string.h>
#include <sys/resource.h>
#include <time.h>

/* The CPU time `speed` and `s_time` measure with (app_tminterval): the
 * process's clock(), all of it as user time; nothing else is asked. */
int getrusage(int who, struct rusage *ru) {
    clock_t c = clock();
    (void)who;
    memset(ru, 0, sizeof *ru);
    ru->ru_utime.tv_sec = c / CLOCKS_PER_SEC;
    ru->ru_utime.tv_usec = (c % CLOCKS_PER_SEC) * (1000000 / CLOCKS_PER_SEC);
    return 0;
}
