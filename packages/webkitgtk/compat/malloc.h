/* myos: WebKit (built as for Linux) and Skia include glibc's <malloc.h>:
 * malloc_usable_size, which mlibc has (its glibc option) without
 * installing the header, and malloc_trim, to give memory back when it
 * runs low. mlibc's allocator gives back what it frees by itself:
 * nothing to trim. */
#ifndef MYOS_COMPAT_MALLOC_H
#define MYOS_COMPAT_MALLOC_H

#include <stddef.h>
#include <stdlib.h>
#include <bits/glibc/glibc_malloc.h>

static inline int malloc_trim(size_t pad)
{
    (void)pad;
    return 0;
}

#endif
