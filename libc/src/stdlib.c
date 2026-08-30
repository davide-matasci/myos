#include <stdlib.h>
#include <unistd.h>

void *malloc(size_t size) {
    if (size == 0) {
        return 0;
    }
    size = (size + 15) & ~((size_t)15);
    return sbrk((intptr_t)size);
}

void free(void *ptr) {
    (void)ptr;
}

void abort(void) {
    _exit(127);
}
