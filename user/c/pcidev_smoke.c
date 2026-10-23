/* pcidev-smoke: boot-CI guest test for /dev/pci (modules/pcidev), a
 * userspace driver for QEMU's `edu` device (vendor 1234, device 11e8; the
 * launcher adds one to every test boot).
 *
 * Finds the device by its `info`, and one a kernel driver drives, whose
 * `ctl` must not open (its info names the driver); holds the edu device
 * (a second open is refused), maps BAR 0 uncached and checks the id, the
 * liveness register (it inverts what is written) and the factorial unit;
 * routes the interrupt
 * and has the device raise one (the `irq` read returns it, the status
 * register names it); where the IOMMU remaps interrupts (`intremap` in
 * /proc/iommu, x86_64), rewrites the device's MSI message to another
 * entry's handle and has it raise again: the message is refused and
 * counted as a fault, the device's own arrives once restored; allocates a
 * DMA buffer and has the device copy a
 * pattern from it into its own memory and back to another part of it,
 * announcing the end with an interrupt. Where the board has an IOMMU
 * (`iommu N` in ctl), a copy to an address outside the buffer is refused:
 * /proc/iommu counts the fault and the memory there stays untouched.
 * Prints [ OK ] pcidev.
 */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

/* edu's registers (QEMU docs/specs/edu.rst). */
#define EDU_ID 0x00
#define EDU_LIVE 0x04
#define EDU_FACT 0x08
#define EDU_STATUS 0x20 /* bit 0: computing */
#define EDU_IRQ_STATUS 0x24
#define EDU_IRQ_RAISE 0x60
#define EDU_IRQ_ACK 0x64
#define EDU_DMA_SRC 0x80
#define EDU_DMA_DST 0x88
#define EDU_DMA_COUNT 0x90
#define EDU_DMA_CMD 0x98 /* bit 0 start, bit 1 edu->RAM, bit 2 interrupt */
#define EDU_DMA_BUF 0x40000 /* the device's own 4 KiB */
#define EDU_DMA_IRQ 0x100   /* the status bit of a finished copy */

static char dir[64];
/* A function a kernel driver drives (`driver ...` in its info). */
static char driven[64];
static volatile uint32_t *regs;

static int fail(const char *what) {
    printf("[ FAIL ] pcidev %s (errno %d)\n", what, errno);
    return 1;
}

static void path(char *out, size_t cap, const char *file) {
    snprintf(out, cap, "/dev/pci/%s/%s", dir, file);
}

static uint32_t rd(unsigned off) {
    return regs[off / 4];
}

static void wr(unsigned off, uint32_t v) {
    regs[off / 4] = v;
}

/* The DMA registers take a whole 64-bit store (a 32-bit one sets only
 * the low half). */
static void wr64(unsigned off, uint64_t v) {
    *(volatile uint64_t *)((volatile uint8_t *)regs + off) = v;
}

/* Wait (up to 5 s) for `bit` of the register at `off` to clear. */
static int settle(unsigned off, uint32_t bit) {
    for (int i = 0; i < 5000; i++) {
        if (!(rd(off) & bit))
            return 1;
        usleep(1000);
    }
    return 0;
}

/* The ctl text; 0 when unreadable. */
static int read_ctl(int fd, char *buf, size_t cap) {
    ssize_t n = pread(fd, buf, cap - 1, 0);
    if (n <= 0)
        return 0;
    buf[n] = 0;
    return 1;
}

/* The value after `key ` in the ctl text, as a number. */
static int ctl_value(const char *text, const char *key, unsigned long long *out) {
    const char *p = strstr(text, key);
    if (!p)
        return 0;
    *out = strtoull(p + strlen(key), NULL, 0);
    return 1;
}

/* Find the edu function's directory under /dev/pci. */
static int find_edu(void) {
    DIR *d = opendir("/dev/pci");
    if (!d)
        return 0;
    struct dirent *e;
    int found = 0;
    while ((e = readdir(d))) {
        if (e->d_name[0] == '.')
            continue;
        char p[128], text[512];
        snprintf(p, sizeof p, "/dev/pci/%s/info", e->d_name);
        int fd = open(p, O_RDONLY);
        if (fd < 0)
            continue;
        if (read_ctl(fd, text, sizeof text) && strstr(text, "id 1234:11e8")) {
            snprintf(dir, sizeof dir, "%s", e->d_name);
            found = 1;
        } else if (strstr(text, "driver ") && !driven[0]) {
            snprintf(driven, sizeof driven, "%s", e->d_name);
        }
        close(fd);
    }
    closedir(d);
    return found;
}

/* The `faults N` count of /proc/iommu; -1 without the file. */
static long long iommu_faults(void) {
    char text[4096];
    int fd = open("/proc/iommu", O_RDONLY);
    if (fd < 0)
        return -1;
    int ok = read_ctl(fd, text, sizeof text);
    close(fd);
    unsigned long long n;
    if (!ok || !ctl_value(text, "faults ", &n))
        return -1;
    return (long long)n;
}

/* Wait for an interrupt on `irq` (up to 5 s); its count, 0 on timeout. */
static uint64_t wait_irq(int irq) {
    struct pollfd p = {.fd = irq, .events = POLLIN};
    if (poll(&p, 1, 5000) != 1)
        return 0;
    uint64_t n = 0;
    if (read(irq, &n, 8) != 8)
        return 0;
    return n;
}

/* Where the IOMMU remaps interrupts (`intremap` in /proc/iommu) and the
 * interrupt is an MSI, the message's address names the table entry that
 * verifies its sender: the message rewritten (through `config`) to the
 * next handle is refused and counted as a fault, nothing arrives on `irq`,
 * and the device's own message arrives again once put back. 0 when the
 * board has neither, or once this passed. */
static int spoof_msi(int irq, const char *ctl_text) {
    char text[4096], p[128];
    int fd = open("/proc/iommu", O_RDONLY);
    if (fd < 0)
        return 0;
    int ok = read_ctl(fd, text, sizeof text);
    close(fd);
    if (!ok || !strstr(text, " intremap ") || !strstr(ctl_text, "irq msi\n"))
        return 0;
    path(p, sizeof p, "config");
    int cfg = open(p, O_RDWR);
    if (cfg < 0)
        return fail("open config for writing");
    /* The MSI capability (id 5) along the list from 0x34; its message
     * address at +4. */
    uint8_t cap = 0, hdr[2] = {0, 0};
    if (pread(cfg, &cap, 1, 0x34) != 1)
        return fail("capability pointer");
    while (cap >= 0x40 && hdr[0] != 0x05) {
        if (pread(cfg, hdr, 2, cap) != 2)
            return fail("capability header");
        if (hdr[0] != 0x05)
            cap = hdr[1];
    }
    if (hdr[0] != 0x05)
        return fail("no msi capability");
    uint32_t addr = 0;
    if (pread(cfg, &addr, 4, cap + 4) != 4)
        return fail("msi address");
    /* The remappable format: bit 4, the handle in bits 19:5 and 2. */
    if (!(addr & 1u << 4))
        return fail("msi message not remappable");
    unsigned handle = (addr >> 5 & 0x7fff) | (addr >> 2 & 1) << 15;
    unsigned other = (handle + 1) % 256;
    uint32_t spoof = 0xFEE00000u | (other & 0x7fff) << 5 | (other >> 15) << 2 | 1u << 4;
    long long before = iommu_faults();
    if (before < 0)
        return fail("/proc/iommu faults");
    if (pwrite(cfg, &spoof, 4, cap + 4) != 4)
        return fail("rewrite the msi address");
    wr(EDU_IRQ_RAISE, 0x5a);
    long long after = before;
    for (int i = 0; i < 300 && after <= before; i++) {
        usleep(10000);
        after = iommu_faults();
    }
    if (after <= before) {
        printf("[ FAIL ] pcidev no iommu fault for a message with another entry's handle (faults %lld)\n", after);
        return 1;
    }
    struct pollfd pd = {.fd = irq, .events = POLLIN};
    if (poll(&pd, 1, 0) != 0)
        return fail("the message with another entry's handle arrived");
    wr(EDU_IRQ_ACK, 0x5a);
    if (pwrite(cfg, &addr, 4, cap + 4) != 4)
        return fail("restore the msi address");
    close(cfg);
    wr(EDU_IRQ_RAISE, 0x5a);
    if (wait_irq(irq) == 0)
        return fail("no interrupt once the message is restored");
    if (rd(EDU_IRQ_STATUS) != 0x5a)
        return fail("interrupt status once the message is restored");
    wr(EDU_IRQ_ACK, 0x5a);
    if (write(irq, "", 1) != 1)
        return fail("unmask after the restored interrupt");
    printf("intremap refused the message naming entry %u: faults %lld -> %lld\n", other, before, after);
    return 0;
}

int main(void) {
    if (!find_edu())
        return fail("no edu device in /dev/pci");
    char p[128], text[512];
    /* The kernel's drivers keep their devices (a virtio NIC or disk, NVMe,
     * xHCI): their ctl does not open. */
    if (!driven[0])
        return fail("no function a kernel driver drives");
    snprintf(p, sizeof p, "/dev/pci/%s/ctl", driven);
    if (open(p, O_RDWR) >= 0)
        return fail("a kernel driver's function opened");
    path(p, sizeof p, "ctl");
    int ctl = open(p, O_RDWR);
    if (ctl < 0)
        return fail("open ctl");
    if (open(p, O_RDWR) >= 0)
        return fail("a second open of ctl was not refused");
    if (write(ctl, "enable", 6) != 6)
        return fail("enable");

    path(p, sizeof p, "config");
    int cfg = open(p, O_RDONLY);
    uint32_t id = 0;
    if (cfg < 0 || pread(cfg, &id, 4, 0) != 4 || id != 0x11e81234)
        return fail("config space id");
    close(cfg);

    path(p, sizeof p, "bar0");
    int bar = open(p, O_RDWR);
    if (bar < 0)
        return fail("open bar0");
    regs = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, bar, 0);
    if (regs == MAP_FAILED)
        return fail("mmap bar0");
    if ((rd(EDU_ID) & 0xFFFF) != 0x00ed)
        return fail("edu id register");
    wr(EDU_LIVE, 0x12345678);
    if (rd(EDU_LIVE) != ~0x12345678u)
        return fail("liveness register");
    wr(EDU_FACT, 5);
    if (!settle(EDU_STATUS, 1) || rd(EDU_FACT) != 120)
        return fail("factorial");

    if (write(ctl, "irq", 3) != 3)
        return fail("route the interrupt");
    path(p, sizeof p, "irq");
    int irq = open(p, O_RDWR);
    if (irq < 0)
        return fail("open irq");
    /* Unmask (an INTx line starts masked), then have the device raise. */
    if (write(irq, "", 1) != 1)
        return fail("unmask");
    wr(EDU_IRQ_RAISE, 0x5a);
    if (wait_irq(irq) == 0)
        return fail("no interrupt");
    if (rd(EDU_IRQ_STATUS) != 0x5a)
        return fail("interrupt status");
    wr(EDU_IRQ_ACK, 0x5a);
    if (write(irq, "", 1) != 1)
        return fail("unmask after the interrupt");
    if (!read_ctl(ctl, text, sizeof text))
        return fail("ctl");
    if (spoof_msi(irq, text))
        return 1;

    if (write(ctl, "dma 1", 5) != 5 || !read_ctl(ctl, text, sizeof text))
        return fail("dma buffer");
    /* Whether the device sees the caches: the buffer is mapped
     * non-cacheable when it does not, so the copies below need no
     * maintenance either way. */
    if (!strstr(text, "dma coherent\n") && !strstr(text, "dma noncoherent\n"))
        return fail("dma coherence in ctl");
    unsigned long long phys = 0;
    if (!ctl_value(text, "dma0 ", &phys) || phys == 0)
        return fail("dma0 in ctl");
    path(p, sizeof p, "dma0");
    int dma = open(p, O_RDWR);
    if (dma < 0)
        return fail("open dma0");
    uint8_t *buf = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, dma, 0);
    if (buf == MAP_FAILED)
        return fail("mmap dma0");
    for (int i = 0; i < 256; i++)
        buf[i] = (uint8_t)(i * 7 + 3);
    /* RAM -> the device's buffer, then back into the second half. */
    wr64(EDU_DMA_SRC, phys);
    wr64(EDU_DMA_DST, EDU_DMA_BUF);
    wr64(EDU_DMA_COUNT, 256);
    wr64(EDU_DMA_CMD, 1);
    if (!settle(EDU_DMA_CMD, 1))
        return fail("dma to the device");
    wr64(EDU_DMA_SRC, EDU_DMA_BUF);
    wr64(EDU_DMA_DST, phys + 2048);
    wr64(EDU_DMA_COUNT, 256);
    wr64(EDU_DMA_CMD, 1 | 2 | 4);
    if (wait_irq(irq) == 0)
        return fail("no interrupt for the dma");
    if (!(rd(EDU_IRQ_STATUS) & EDU_DMA_IRQ))
        return fail("dma interrupt status");
    wr(EDU_IRQ_ACK, EDU_DMA_IRQ);
    for (int i = 0; i < 256; i++) {
        if (buf[2048 + i] != (uint8_t)(i * 7 + 3))
            return fail("dma copy");
    }
    /* Confined: the page after the buffer is not the device's. A copy from
     * the device's memory there is refused by the IOMMU (counted in
     * /proc/iommu); the buffer, which it would have reached one page
     * further without one, is untouched. */
    unsigned long long domain = 0;
    if (!read_ctl(ctl, text, sizeof text))
        return fail("ctl text");
    if (ctl_value(text, "iommu ", &domain)) {
        long long before = iommu_faults();
        if (before < 0)
            return fail("/proc/iommu");
        memset(buf, 0, 4096);
        wr64(EDU_DMA_SRC, EDU_DMA_BUF);
        wr64(EDU_DMA_DST, phys + 4096);
        wr64(EDU_DMA_COUNT, 256);
        wr64(EDU_DMA_CMD, 1 | 2);
        if (!settle(EDU_DMA_CMD, 1))
            return fail("dma outside the buffer did not end");
        /* The unit's fault event is an interrupt on another CPU, maybe:
         * give it a moment. */
        long long after = before;
        for (int i = 0; i < 300 && after <= before; i++) {
            usleep(10000);
            after = iommu_faults();
        }
        if (after <= before) {
            printf("[ FAIL ] pcidev no iommu fault for a dma outside the buffer (faults %lld)\n", after);
            return 1;
        }
        for (int i = 0; i < 4096; i++)
            if (buf[i]) {
                printf("[ FAIL ] pcidev the refused dma landed in the buffer\n");
                return 1;
            }
        printf("iommu domain %llu refused the dma: faults %lld -> %lld\n", domain, before, after);
    } else {
        printf("no iommu: the dma is unconfined\n");
    }
    close(ctl);
    path(p, sizeof p, "ctl");
    ctl = open(p, O_RDWR);
    if (ctl < 0)
        return fail("ctl after its holder let go");
    close(ctl);
    printf("[ OK ] pcidev\n");
    return 0;
}
