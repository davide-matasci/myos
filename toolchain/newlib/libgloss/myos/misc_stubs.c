#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <signal.h>
#include <unistd.h>
#include <sys/time.h>

#include "myos_syscalls.h"

void sync(void) {
}

char *getpass(const char *prompt) {
    static char buf[128];
    size_t i = 0;

    /* The prompt in one write: a byte at a time, a kernel line on another
     * CPU (a module's status) landed between its letters (issue #632). */
    if (prompt != NULL) {
        write(2, prompt, strlen(prompt));
    }
    for (;;) {
        char c = 0;
        ssize_t n = read(0, &c, 1);
        if (n <= 0) {
            break;
        }
        if (c == '\n' || c == '\r') {
            break;
        }
        if (i + 1 < sizeof(buf)) {
            buf[i++] = c;
        }
    }
    buf[i] = '\0';
    write(2, "\n", 1);
    return buf;
}
