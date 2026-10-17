/* jit_smoke: a JIT's pattern from two threads (kernel/src/user/aspace.rs,
 * docs/threads.md). One thread makes a range of pages not touched yet
 * executable with mprotect while the other reads them, so that some are
 * faulted in during the call; then code is written into every page and
 * run. A page the reader faulted in while mprotect walked the range used
 * to keep the protection the range had before (writable, not executable),
 * and the first fetch from it killed the process (issue #492). Fifty
 * rounds of 1024 pages. Prints [ OK ] jit.
 */
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>

#define PAGE 4096
#define PAGES 1024
#define ROUNDS 50

static unsigned char *region;
/* 0: no range; 1: the reader touches the range; 2: the reader stops. */
static atomic_int phase;
static atomic_long touches;

/* Touch pages of the range, at random, while the main thread changes its
 * protection: each first touch faults the page in. */
static void *reader(void *arg) {
    (void)arg;
    unsigned seed = 1;
    while (atomic_load(&phase) != 2) {
        if (atomic_load(&phase) == 1) {
            seed = seed * 1103515245u + 12345u;
            volatile unsigned char *p = region + ((seed >> 8) % PAGES) * PAGE;
            (void)*p;
            atomic_fetch_add(&touches, 1);
        }
    }
    return NULL;
}

/* `return n` at `p`: the page's index comes back from a call. */
static void put_code(unsigned char *p, int n) {
#if defined(__x86_64__)
    p[0] = 0xb8; /* mov eax, imm32 */
    memcpy(p + 1, &n, 4);
    p[5] = 0xc3; /* ret */
#elif defined(__aarch64__)
    uint32_t insn[2] = {0x52800000u | ((uint32_t)n << 5), 0xd65f03c0u}; /* movz w0, #n; ret */
    memcpy(p, insn, sizeof insn);
#elif defined(__riscv)
    uint32_t insn[2] = {0x00000513u | ((uint32_t)n << 20), 0x00008067u}; /* addi a0, zero, n; ret */
    memcpy(p, insn, sizeof insn);
#else
#error "no code for this architecture"
#endif
    __builtin___clear_cache((char *)p, (char *)p + 16);
}

int main(void) {
    int failures = 0;
    for (int round = 0; round < ROUNDS; round++) {
        region = mmap(NULL, PAGES * PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (region == MAP_FAILED) {
            printf("jit_smoke: mmap failed\n");
            return 1;
        }
        pthread_t t;
        atomic_store(&phase, 1);
        if (pthread_create(&t, NULL, reader, NULL) != 0) {
            printf("jit_smoke: pthread_create failed\n");
            return 1;
        }
        /* The reader faults pages in while this walks the range. */
        if (mprotect(region, PAGES * PAGE, PROT_READ | PROT_WRITE | PROT_EXEC) != 0) {
            printf("jit_smoke: mprotect failed\n");
            return 1;
        }
        atomic_store(&phase, 2);
        pthread_join(t, NULL);
        for (int i = 0; i < PAGES; i++) {
            put_code(region + i * PAGE, i);
        }
        for (int i = 0; i < PAGES; i++) {
            int (*fn)(void) = (int (*)(void))(region + i * PAGE);
            int got = fn();
            if (got != i) {
                printf("jit_smoke: round %d page %d returned %d\n", round, i, got);
                failures++;
            }
        }
        munmap(region, PAGES * PAGE);
        atomic_store(&phase, 0);
    }
    if (atomic_load(&touches) == 0) {
        printf("jit_smoke: the reader never ran\n");
        failures++;
    }
    if (failures) {
        return 1;
    }
    printf("[ OK ] jit\n");
    return 0;
}
