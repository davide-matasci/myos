/* syslog for myos: no log daemon, each message is one line written to the
 * console (/dev/console/data), "ident[pid]: message", and to stderr as well
 * with LOG_PERROR. %m in the format is strerror(errno). */

#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include <sys/syslog.h>
#include <unistd.h>

#define MYOS_SYSLOG_CONSOLE "/dev/console/data"

static const char *log_ident;
static int log_option;
static int log_mask = 0xff;

void openlog(const char *ident, int option, int facility) {
    (void)facility;
    log_ident = ident;
    log_option = option;
}

void closelog(void) {
    log_ident = NULL;
    log_option = 0;
}

int setlogmask(int mask) {
    int old = log_mask;
    if (mask != 0) {
        log_mask = mask;
    }
    return old;
}

/* The format with each %m replaced by the error text (%% left alone). */
static void expand_m(const char *format, int err, char *out, size_t size) {
    const char *text = strerror(err);
    size_t o = 0;
    for (const char *p = format; *p != '\0' && o + 1 < size; p++) {
        if (p[0] == '%' && p[1] == 'm') {
            for (const char *t = text; *t != '\0' && o + 1 < size; t++) {
                /* a % in the text must not start a conversion */
                if (*t == '%' && o + 2 < size) {
                    out[o++] = '%';
                }
                out[o++] = *t;
            }
            p++;
        } else {
            if (p[0] == '%' && p[1] == '%' && o + 2 < size) {
                out[o++] = *p++;
            }
            out[o++] = *p;
        }
    }
    out[o] = '\0';
}

void vsyslog(int priority, const char *format, va_list ap) {
    char fmt[512];
    char line[1024];
    int err = errno;
    int n = 0;
    int fd;

    if ((LOG_MASK(LOG_PRI(priority)) & log_mask) == 0 || format == NULL) {
        return;
    }
    if (log_ident != NULL) {
        n = snprintf(line, sizeof line, "%s", log_ident);
    }
    if (log_option & LOG_PID) {
        n += snprintf(line + n, sizeof line - n, "[%d]", (int)getpid());
    }
    if (n > 0) {
        n += snprintf(line + n, sizeof line - n, ": ");
    }
    expand_m(format, err, fmt, sizeof fmt);
    if (n < (int)sizeof line) {
        n += vsnprintf(line + n, sizeof line - n, fmt, ap);
    }
    if (n > (int)sizeof line - 2) {
        n = sizeof line - 2;
    }
    /* one line per message: the text's own newline, or ours */
    if (n == 0 || line[n - 1] != '\n') {
        line[n++] = '\n';
    }
    if ((fd = open(MYOS_SYSLOG_CONSOLE, O_WRONLY)) >= 0) {
        write(fd, line, n);
        close(fd);
    }
    if (log_option & LOG_PERROR) {
        write(STDERR_FILENO, line, n);
    }
    errno = err;
}

void syslog(int priority, const char *format, ...) {
    va_list ap;
    va_start(ap, format);
    vsyslog(priority, format, ap);
    va_end(ap);
}
