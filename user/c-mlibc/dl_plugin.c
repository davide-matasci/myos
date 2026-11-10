/* dl_plugin.so: what dl_smoke loads with dlopen (dl_smoke.c). It calls
 * into libdlsmoke.so, which the program has loaded already, and into the
 * program itself (dl_smoke_host, exported with --export-dynamic). */

int dl_lib_length(const char *s);
int dl_smoke_host(int x);

static int loaded;
static __thread int plugin_tls = 40;

__attribute__((constructor)) static void construct(void) {
    loaded = 1;
}

/* 42 when the constructor ran, the TLS block is this thread's own and
 * both calls out of the object resolve. */
int plugin_answer(void) {
    plugin_tls += 1;
    return loaded + plugin_tls + dl_lib_length("") + dl_smoke_host(0);
}

int plugin_data = 1234;
