/* libdlsmoke.so: the shared object dl_smoke names in DT_NEEDED and its
 * plugin calls back into (dl_smoke.c). */
#include <string.h>

static int constructed;
__thread int dl_lib_tls = 7;

__attribute__((constructor)) static void construct(void) {
    constructed = 1;
}

int dl_lib_constructed(void) {
    return constructed;
}

/* The libc's, through this object's own PLT. */
int dl_lib_length(const char *s) {
    return (int)strlen(s);
}

int *dl_lib_tls_address(void) {
    return &dl_lib_tls;
}
