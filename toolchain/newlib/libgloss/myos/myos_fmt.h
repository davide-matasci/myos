/* Tiny decimal/string helpers — avoid snprintf (soft-float vfprintf on riscv). */
#ifndef MYOS_FMT_H
#define MYOS_FMT_H

#include <stddef.h>

static inline int myos_u16_dec(char *dst, size_t cap, unsigned v) {
    char tmp[6];
    int i = 6;
    int n;
    if (v > 65535u) {
        v = 65535u;
    }
    if (v == 0) {
        tmp[--i] = '0';
    } else {
        while (v != 0 && i > 0) {
            tmp[--i] = (char)('0' + (v % 10u));
            v /= 10u;
        }
    }
    n = 6 - i;
    if ((size_t)n >= cap) {
        return -1;
    }
    for (int k = 0; k < n; k++) {
        dst[k] = tmp[i + k];
    }
    dst[n] = '\0';
    return n;
}

static inline int myos_u8_dec(char *dst, size_t cap, unsigned v) {
    return myos_u16_dec(dst, cap, v & 0xffu);
}

static inline int myos_cpy(char *dst, size_t cap, const char *src) {
    size_t n = 0;
    while (src[n] != '\0') {
        n++;
    }
    if (n >= cap) {
        return -1;
    }
    for (size_t i = 0; i < n; i++) {
        dst[i] = src[i];
    }
    dst[n] = '\0';
    return (int)n;
}

#endif
