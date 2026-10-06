/*
 * Exec-loader hardening probe: write a tiny ELF whose single PT_LOAD claims a
 * virtual address at the very top of the address space (small memsz, so the
 * image span stays tiny and slips the loader's span cap), then exec it. Before
 * the fix the loader computed `load_base - p_vaddr`, which underflowed and
 * aborted the kernel. After the fix span_from rejects the segment (its end is
 * past IMAGE_VADDR_MAX) and exec fails cleanly, so the machine survives.
 *
 * The pass is simply that this program keeps running and reports it: a kernel
 * that panicked would never print the line.
 */
#define _GNU_SOURCE 1
#include <fcntl.h>
#include <stdint.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

#if defined(__x86_64__)
#define E_MACHINE 62
#elif defined(__aarch64__)
#define E_MACHINE 183
#elif defined(__riscv)
#define E_MACHINE 243
#else
#error "unknown arch"
#endif

static void put_le(unsigned char *p, uint64_t v, int n) {
    for (int i = 0; i < n; i++) {
        p[i] = (unsigned char)(v >> (8 * i));
    }
}

int main(void) {
    unsigned char elf[120];
    memset(elf, 0, sizeof elf);
    /* Ehdr */
    elf[0] = 0x7f; elf[1] = 'E'; elf[2] = 'L'; elf[3] = 'F';
    elf[4] = 2;   /* ELFCLASS64 */
    elf[5] = 1;   /* ELFDATA2LSB */
    elf[6] = 1;   /* EI_VERSION */
    put_le(elf + 16, 2, 2);          /* e_type = ET_EXEC */
    put_le(elf + 18, E_MACHINE, 2);  /* e_machine */
    put_le(elf + 20, 1, 4);          /* e_version */
    put_le(elf + 24, 0x1000, 8);     /* e_entry */
    put_le(elf + 32, 64, 8);         /* e_phoff */
    put_le(elf + 52, 64, 2);         /* e_ehsize */
    put_le(elf + 54, 56, 2);         /* e_phentsize */
    put_le(elf + 56, 1, 2);          /* e_phnum */
    put_le(elf + 58, 64, 2);         /* e_shentsize (loader wants >= 64) */
    /* One PT_LOAD at the top of the address space. */
    unsigned char *ph = elf + 64;
    put_le(ph + 0, 1, 4);                       /* p_type = PT_LOAD */
    put_le(ph + 4, 5, 4);                       /* p_flags = R|X */
    put_le(ph + 8, 0, 8);                       /* p_offset */
    put_le(ph + 16, 0xfffffffffffff000ULL, 8);  /* p_vaddr (top of space) */
    put_le(ph + 32, 0, 8);                      /* p_filesz */
    put_le(ph + 40, 0x1000, 8);                 /* p_memsz */
    put_le(ph + 48, 0x1000, 8);                 /* p_align */

    const char *path = "/tmp/crash.elf";
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0755);
    if (fd < 0 || write(fd, elf, sizeof elf) != (ssize_t)sizeof elf) {
        write(1, "[ FAIL ] crash_elf: setup\n", 26);
        return 1;
    }
    close(fd);

    pid_t pid = fork();
    if (pid == 0) {
        char *argv[] = {(char *)path, 0};
        execv(path, argv);
        _exit(7); /* exec must fail, not crash */
    }
    int st;
    waitpid(pid, &st, 0);
    unlink(path);
    /* Reaching here at all means the kernel did not panic on the exec. */
    write(1, "[ OK ] crash_elf: kernel survived hostile ELF\n", 46);
    return 0;
}
