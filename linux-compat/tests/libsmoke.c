/* Shared library linked into linux-dyn (DT_NEEDED): code, data, a
 * function-pointer table (relocations) and a thread-local. */
int smoke_counter = 40;
static __thread int smoke_tls = 7;

int smoke_add(int a, int b) {
    return a + b;
}

static int twice(int x) {
    return 2 * x;
}

int (*const smoke_ops[])(int) = {twice};

int smoke_bump(void) {
    return ++smoke_counter;
}

int smoke_tls_next(void) {
    return ++smoke_tls;
}
