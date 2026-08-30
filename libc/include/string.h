#ifndef STRING_H
#define STRING_H

#include <stddef.h>

size_t strlen(const char *s);
int strcmp(const char *a, const char *b);
char *strcpy(char *dst, const char *src);
void *memset(void *s, int c, size_t n);
void *memcpy(void *dst, const void *src, size_t n);

#endif
