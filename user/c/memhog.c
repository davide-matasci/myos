/*
 * Memory-exhaustion probe: mmap and touch anonymous memory in chunks until a
 * cap is reached or memory runs out. The kernel must either serve every page
 * (and reclaim it all when we exit) or, when memory runs low, fail the fault
 * so this process dies — never abort the kernel.
 *
 * Touching in chunks (rather than one huge mmap, which the fragmented mmap
 * window can refuse) keeps allocating until physical RAM is actually gone, so
 * on a small machine this reaches the out-of-memory path. If a touch faults,
 * the process is killed by SIGSEGV mid-loop; the kernel stays up, which is the
 * point. Prints progress so a run is legible. Run by hand / wired locally.
 */
#define _GNU_SOURCE 1
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

static void puts_raw(const char *s) { write(1, s, strlen(s)); }

static void put_num(long n) {
    char b[24];
    int i = sizeof b;
    unsigned long v = (unsigned long)n;
    b[--i] = '\n';
    if (v == 0) {
        b[--i] = '0';
    }
    while (v && i > 0) {
        b[--i] = '0' + (v % 10);
        v /= 10;
    }
    write(1, b + i, sizeof b - i);
}

#define PAGE 4096
#define MIB (1024UL * 1024UL)
#define CHUNK (16UL * MIB)

int main(int argc, char **argv) {
    /* Cap in MiB (default 4096: effectively "until RAM runs out"). */
    unsigned long cap = 4096;
    if (argc > 1) {
        cap = 0;
        for (const char *p = argv[1]; *p >= '0' && *p <= '9'; p++) {
            cap = cap * 10 + (unsigned long)(*p - '0');
        }
    }
    unsigned long cap_bytes = cap * MIB;
    unsigned long touched = 0;
    while (touched < cap_bytes) {
        unsigned char *m = mmap(0, CHUNK, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (m == MAP_FAILED) {
            puts_raw("memhog: mmap full at MiB=");
            put_num((long)(touched / MIB));
            break; /* virtual window full, not a RAM shortage */
        }
        /* Touch every page; a fault the kernel cannot satisfy kills us here. */
        for (unsigned long off = 0; off < CHUNK; off += PAGE) {
            m[off] = 1;
        }
        touched += CHUNK;
        if (touched % (128 * MIB) == 0) {
            puts_raw("memhog: touched MiB=");
            put_num((long)(touched / MIB));
        }
    }
    puts_raw("memhog: done MiB=");
    put_num((long)(touched / MIB));
    return 0;
}
