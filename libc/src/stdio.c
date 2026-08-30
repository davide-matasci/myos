#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int puts(const char *s) {
    size_t n = strlen(s);
    if (write(1, s, n) < 0) {
        return -1;
    }
    return write(1, "\n", 1) < 0 ? -1 : (int)n + 1;
}

static void put_u32(unsigned n) {
    char buf[12];
    int i = 0;
    if (n == 0) {
        write(1, "0", 1);
        return;
    }
    while (n > 0) {
        buf[i++] = (char)('0' + (n % 10));
        n /= 10;
    }
    while (i > 0) {
        char c = buf[--i];
        write(1, &c, 1);
    }
}

int printf(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int out = 0;
    for (const char *p = fmt; *p; p++) {
        if (p[0] == '%' && p[1] == 's') {
            const char *s = va_arg(ap, const char *);
            size_t n = strlen(s);
            write(1, s, n);
            out += (int)n;
            p++;
        } else if (p[0] == '%' && p[1] == 'd') {
            put_u32((unsigned)va_arg(ap, int));
            out += 1;
            p++;
        } else if (p[0] == '%' && p[1] == '%') {
            write(1, "%", 1);
            out += 1;
            p++;
        } else {
            write(1, p, 1);
            out += 1;
        }
    }
    va_end(ap);
    return out;
}
