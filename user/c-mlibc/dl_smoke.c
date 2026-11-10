/*
 * Shared objects (docs/mlibc.md "Shared objects"): a program linked
 * against the shared mlibc, started by the kernel through its dynamic
 * linker (/lib/ld.so, PT_INTERP).
 *
 * - libdlsmoke.so is named in DT_NEEDED and found beside the program
 *   (DT_RUNPATH $ORIGIN): its constructor ran before main, its functions
 *   and its thread-local variable resolve, the variable one per thread.
 * - dl_plugin.so (argv[1]) is loaded with dlopen: dlsym finds its function
 *   and its data, the function calls into libdlsmoke.so and back into the
 *   program, and its own thread-local works in two threads. A second
 *   dlopen gives the same handle; a missing object or symbol gives NULL
 *   and a dlerror. dlclose succeeds and leaves the object loaded (mlibc
 *   unloads nothing before the program ends).
 * - dladdr names the object an address is in.
 *
 * Prints one `[ OK ] dl` / `[ FAIL ] dl ...` line.
 */
#define _GNU_SOURCE 1
#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>

int dl_lib_constructed(void);
int dl_lib_length(const char *s);
int *dl_lib_tls_address(void);
extern __thread int dl_lib_tls;

/* What the plugin calls back. */
int dl_smoke_host(int x) {
    return x;
}

static int (*answer)(void);

static void *other_thread(void *arg) {
    int *main_tls = arg;
    /* This thread's own copies: the initial values, at other addresses. */
    if (dl_lib_tls != 7 || dl_lib_tls_address() == main_tls || &dl_lib_tls != dl_lib_tls_address()) {
        return "the needed object's TLS in a second thread";
    }
    if (answer() != 42) {
        return "the plugin's TLS in a second thread";
    }
    return NULL;
}

static const char *run(const char *plugin) {
    if (!dl_lib_constructed()) {
        return "the needed object's constructor did not run";
    }
    if (dl_lib_length("shared") != 6) {
        return "a call into the needed object";
    }
    dl_lib_tls = 8;
    if (*dl_lib_tls_address() != 8) {
        return "the needed object's TLS";
    }

    void *h = dlopen(plugin, RTLD_NOW);
    if (h == NULL) {
        fprintf(stderr, "dlopen: %s\n", dlerror());
        return "dlopen";
    }
    if (dlopen(plugin, RTLD_NOW) != h) {
        return "a second dlopen gave another handle";
    }
    dlclose(h);
    answer = (int (*)(void))dlsym(h, "plugin_answer");
    int *data = dlsym(h, "plugin_data");
    if (answer == NULL || data == NULL) {
        return "dlsym";
    }
    if (*data != 1234) {
        return "the plugin's data";
    }
    if (answer() != 42) {
        return "the plugin's function";
    }
    if (dlsym(h, "no_such_symbol") != NULL || dlerror() == NULL) {
        return "dlsym of a missing symbol";
    }
    Dl_info info;
    if (dladdr((void *)answer, &info) == 0 || info.dli_fname == NULL || strstr(info.dli_fname, "dl_plugin") == NULL) {
        return "dladdr";
    }

    pthread_t t;
    void *err = NULL;
    if (pthread_create(&t, NULL, other_thread, dl_lib_tls_address()) != 0 || pthread_join(t, &err) != 0) {
        return "pthread_create";
    }
    if (err != NULL) {
        return err;
    }
    /* The second thread's count was its own: this one's goes on from 41. */
    if (answer() != 43) {
        return "the plugin's TLS after the second thread";
    }

    if (dlclose(h) != 0 || answer() != 44) {
        return "dlclose";
    }
    if (dlopen("/no/such/object.so", RTLD_NOW) != NULL || dlerror() == NULL) {
        return "dlopen of a missing object";
    }
    return NULL;
}

int main(int argc, char **argv) {
    const char *e = argc > 1 ? run(argv[1]) : "usage: dl_smoke PLUGIN";
    if (e != NULL) {
        printf("[ FAIL ] dl: %s\n", e);
        return 1;
    }
    printf("[ OK ] dl\n");
    return 0;
}
