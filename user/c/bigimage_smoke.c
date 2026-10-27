/* bigimage_smoke: a native program too large to read whole, which exec maps
 * from its file (docs/linux-compat.md, the exec limits).
 *
 * Its 20 MiB .bss puts its image over the kernel's limit for one read
 * whole (MAX_ELF_PAGES), so exec maps it and relocates it in place: the
 * pointers in its tables (relocated words) lead to their strings and
 * variables, its data has its initial values, the .bss is zero and
 * writable, the heap grows, a forked child's writes stay its own, and the
 * program runs itself again by exec. Exits 0 when all hold, else prints
 * what failed.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

#define BIG (20 << 20)

static char big[BIG];
static int counter = 42;
static int one = 1, two = 2;
/* Relocated words: pointers, in .data.rel.ro and .data. */
static const char *const names[] = {"alpha", "beta", "gamma"};
static int *ptrs[] = {&one, &two, &counter};

static int fail(const char *what) {
    printf("bigimage_smoke: %s\n", what);
    return 1;
}

/* What every run checks: the relocated tables, the data, the .bss. */
static int check(void) {
    if (strcmp(names[0], "alpha") || strcmp(names[2], "gamma")) {
        return fail("relocated string table");
    }
    if (*ptrs[0] != 1 || *ptrs[1] != 2 || ptrs[2] != &counter) {
        return fail("relocated pointer table");
    }
    if (counter != 42) {
        return fail("initial data");
    }
    for (size_t i = 0; i < BIG; i += 4096) {
        if (big[i]) {
            return fail(".bss not zero");
        }
    }
    big[0] = 1;
    big[BIG - 1] = 2;
    if (big[0] != 1 || big[BIG - 1] != 2) {
        return fail(".bss not writable");
    }
    char *heap = malloc(1 << 20);
    if (!heap) {
        return fail("malloc");
    }
    memset(heap, 0x5a, 1 << 20);
    free(heap);
    return 0;
}

int main(int argc, char **argv) {
    if (check()) {
        return 1;
    }
    if (argc > 1 && strcmp(argv[1], "again") == 0) {
        return 0;
    }
    pid_t pid = fork();
    if (pid < 0) {
        return fail("fork");
    }
    if (pid == 0) {
        counter = 7;
        big[4096] = 3;
        _exit(counter == 7 && big[4096] == 3 ? 0 : 1);
    }
    int status;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status)) {
        return fail("child");
    }
    if (counter != 42 || big[4096] != 0) {
        return fail("child's writes reached the parent");
    }
    pid = fork();
    if (pid == 0) {
        execl(argv[0], argv[0], "again", (char *)NULL);
        _exit(127);
    }
    if (pid < 0 || waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status)) {
        return fail("exec of itself");
    }
    return 0;
}
