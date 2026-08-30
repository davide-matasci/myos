/* Minimal sbase util.h for myos (no stdio/regex). */
#pragma once

#include <stddef.h>
#include <stdarg.h>
#include <sys/types.h>

#include "arg.h"
#include "compat.h"

extern char *argv0;

void eprintf(const char *, ...);
void enprintf(int, const char *, ...);
void weprintf(const char *, ...);
void xvprintf(const char *, va_list);

ssize_t writeall(int, const void *, size_t);
int concat(int, const char *, int, const char *);
void putword(const char *);
void *ereallocarray(void *, size_t, size_t);
char *estrdup(const char *);
