/* See LICENSE file for copyright and license details. */
#include <errno.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "util.h"

char *argv0;

static void
write_str(int fd, const char *s)
{
	if (s && *s)
		writeall(fd, s, strlen(s));
}

static void
vfmtfd(int fd, const char *fmt, va_list ap)
{
	while (*fmt) {
		if (fmt[0] == '%' && fmt[1] == 's') {
			write_str(fd, va_arg(ap, char *));
			fmt += 2;
		} else {
			writeall(fd, fmt, 1);
			fmt++;
		}
	}
}

void
eprintf(const char *fmt, ...)
{
	va_list ap;

	va_start(ap, fmt);
	xvprintf(fmt, ap);
	va_end(ap);

	exit(1);
}

void
enprintf(int status, const char *fmt, ...)
{
	va_list ap;

	va_start(ap, fmt);
	xvprintf(fmt, ap);
	va_end(ap);

	exit(status);
}

void
weprintf(const char *fmt, ...)
{
	va_list ap;

	va_start(ap, fmt);
	xvprintf(fmt, ap);
	va_end(ap);
}

void
xvprintf(const char *fmt, va_list ap)
{
	if (argv0 && strncmp(fmt, "usage", strlen("usage"))) {
		write_str(2, argv0);
		writeall(2, ": ", 2);
	}
	vfmtfd(2, fmt, ap);
	if (fmt[0] && fmt[strlen(fmt) - 1] == ':') {
		writeall(2, " ", 1);
		write_str(2, strerror(errno));
	}
}
