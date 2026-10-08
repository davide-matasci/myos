/*
 * get-myos --upgrade and --install: the boot disk (docs/install.md).
 *
 * The boot disk's ESP has two slots, boot/a/ and boot/b/, each a kernel, its
 * initramfs and a `version` (the release line of /lib/myos-release), and one
 * boot/limine/limine.conf with an entry per filled slot, the default first;
 * an entry's `cmdline: slot=X` tells the kernel which it booted
 * (/proc/cmdline).
 *
 * --upgrade downloads the release's kernel and initramfs (<arch>-boot.txt on
 * the mirror: their sizes and SHA-256) into memory, checks them, writes them
 * into the slot that is not running, reads them back from the disk (unmounted and mounted again in between)
 * against the checksums, writes the slot's version, and only then rewrites
 * limine.conf with that slot first and the running one as the fallback: a
 * failure on the way leaves the running slot and the config as they were.
 * A mirror whose release is not newer than the running slot's is left alone
 * unless forced (-f).
 *
 * --install DISK lays the boot disk out on DISK (the running boot disk, or
 * one with a mounted partition, is refused): a GPT with a BIOS boot
 * partition (1 MiB, empty: the BIOS stage is not written, so the disk boots
 * by UEFI), the ESP (512 MiB, mkfs.fat) and the data partition (the rest,
 * mkfs.ext2); copies Limine from the running ESP onto the new one, and
 * upgrades its slot a.
 */
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <dirent.h>
#include <unistd.h>

#include "boot.h"
#include "pkgtools.h"

int mount(const char *source, const char *target, const char *fstype, ...);

#define ESP_TYPE "c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
#define RUNNING_ESP "/tmp/get-myos-esp"
#define NEW_ESP "/tmp/get-myos-new-esp"
#define SECTOR 512ULL
#define MIB (1024ULL * 1024 / SECTOR)

/* ---- small helpers ---------------------------------------------------- */

/* Run a program (looked up in PATH) to the end: 0 when it exited 0. */
static int run(char *const argv[]) {
    pid_t pid = fork();
    if (pid < 0) {
        return -1;
    }
    if (pid == 0) {
        execvp(argv[0], argv);
        _exit(127);
    }
    int status = 0;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status)) {
        return -1;
    }
    return WEXITSTATUS(status) == 0 ? 0 : -1;
}

static int umount_dir(const char *dir) {
    char *argv[] = {"umount", (char *)dir, NULL};
    return run(argv);
}

/* The first line of a file into buf (without its newline); -1 if none. */
static int read_line(const char *path, char *buf, size_t cap) {
    FILE *f = fopen(path, "r");
    if (f == NULL) {
        return -1;
    }
    buf[0] = '\0';
    char *got = fgets(buf, (int)cap, f);
    fclose(f);
    if (got == NULL) {
        return -1;
    }
    buf[strcspn(buf, "\n")] = '\0';
    return 0;
}

static int write_file(const char *path, const char *data, size_t n) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        return -1;
    }
    size_t done = 0;
    while (done < n) {
        ssize_t w = write(fd, data + done, n - done);
        if (w <= 0) {
            close(fd);
            return -1;
        }
        done += (size_t)w;
    }
    return close(fd);
}

/* The SHA-256 of a file in hex[65]; -1 if it cannot be read. */
static int sha256_file(const char *path, char *hex, long long *size) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return -1;
    }
    sha256 s;
    sha256_init(&s);
    static uint8_t buf[65536];
    long long total = 0;
    ssize_t n;
    while ((n = read(fd, buf, sizeof buf)) > 0) {
        sha256_update(&s, buf, (size_t)n);
        total += n;
    }
    close(fd);
    if (n < 0) {
        return -1;
    }
    sha256_hex(&s, hex);
    *size = total;
    return 0;
}

/* The value of `key=` in a line of "key=value" words into out[cap]. */
static void word_value(const char *line, const char *key, char *out, size_t cap) {
    out[0] = '\0';
    size_t kl = strlen(key);
    for (const char *p = line; *p != '\0'; p++) {
        if ((p == line || p[-1] == ' ') && strncmp(p, key, kl) == 0 && p[kl] == '=') {
            copy_field(out, cap, p + kl + 1, strcspn(p + kl + 1, " \n"));
            return;
        }
    }
}

/* ---- the running system's boot disk ------------------------------------ */

/* The slot this boot came from (`slot=a` in /proc/cmdline): 'a', 'b', or
 * 0 when the boot entry did not say (a disk written before the slots). */
static char running_slot(void) {
    char line[256], slot[4];
    if (read_line("/proc/cmdline", line, sizeof line) != 0) {
        return 0;
    }
    word_value(line, "slot", slot, sizeof slot);
    return (slot[0] == 'a' || slot[0] == 'b') && slot[1] == '\0' ? slot[0] : 0;
}

/* Mount the running boot disk's ESP at RUNNING_ESP: of the ESP partitions
 * (/proc/partitions), the one whose running slot has this system's release
 * (several disks may carry an ESP). Its partition name ("nvme2n1/p2") into
 * name[cap]. */
static int mount_running_esp(char slot, char *name, size_t cap) {
    char release[128], line[512];
    if (read_line("/lib/myos-release", release, sizeof release) != 0) {
        return die("no /lib/myos-release", NULL);
    }
    FILE *f = fopen("/proc/partitions", "r");
    if (f == NULL) {
        return die("no /proc/partitions", NULL);
    }
    mkdirs(RUNNING_ESP, 1);
    int found = -1;
    while (found != 0 && fgets(line, sizeof line, f) != NULL) {
        char part[64], dev[80], path[128], version[128];
        if (strstr(line, " " ESP_TYPE " ") == NULL) {
            continue;
        }
        copy_field(part, sizeof part, line, strcspn(line, " "));
        strcpy(dev, "/dev/");
        strcat(dev, part);
        if (mount(dev, RUNNING_ESP, "fat") != 0) {
            continue;
        }
        strcpy(path, RUNNING_ESP "/boot/x/version");
        path[strlen(RUNNING_ESP) + 6] = slot;
        if (read_line(path, version, sizeof version) == 0 && strcmp(version, release) == 0) {
            copy_field(name, cap, part, strlen(part));
            found = 0;
        } else {
            umount_dir(RUNNING_ESP);
        }
    }
    fclose(f);
    if (found != 0) {
        say("no ESP has slot ", (char[]){slot, '\0'}, " at this system's release (/lib/myos-release)");
        return 1;
    }
    return 0;
}

/* ---- the mirror's boot files -------------------------------------------- */

typedef struct {
    char header[256];  /* "release=... commit=... abi=..." */
    char release[32];
    char csum[2][72], file[2][96];
    long long size[2];
} boot_files;

/* <arch>-boot.txt: the header, then "kernel SIZE SHA256 FILE" and the same
 * for the initramfs. */
static int fetch_boot_list(const char *list_path, boot_files *b) {
    char line[512];
    FILE *f = fopen(list_path, "r");
    if (f == NULL) {
        return -1;
    }
    memset(b, 0, sizeof *b);
    int have = 0;
    while (fgets(line, sizeof line, f) != NULL) {
        line[strcspn(line, "\n")] = '\0';
        if (strncmp(line, "# myos ", 7) == 0) {
            copy_field(b->header, sizeof b->header, line + 7, strlen(line + 7));
            word_value(b->header, "release", b->release, sizeof b->release);
            continue;
        }
        int i = strncmp(line, "kernel ", 7) == 0 ? 0 : strncmp(line, "initramfs ", 10) == 0 ? 1 : -1;
        if (i < 0) {
            continue;
        }
        char *p = strchr(line, ' ') + 1;
        b->size[i] = atoll(p);
        p = strchr(p, ' ');
        if (p == NULL) {
            break;
        }
        p++;
        copy_field(b->csum[i], sizeof b->csum[i], p, strcspn(p, " "));
        p = strchr(p, ' ');
        if (p == NULL) {
            break;
        }
        copy_field(b->file[i], sizeof b->file[i], p + 1, strlen(p + 1));
        have |= 1 << i;
    }
    fclose(f);
    return have == 3 && b->header[0] != '\0' ? 0 : -1;
}

/* ---- writing a slot ----------------------------------------------------- */

static const char *NAMES[2] = {"kernel", "initramfs"};

/* ESP-relative path of a slot's file into out: "<esp>/boot/<slot>/<name>". */
static void slot_path(char *out, const char *esp, char slot, const char *name) {
    strcpy(out, esp);
    strcat(out, "/boot/x/");
    out[strlen(esp) + 6] = slot;
    strcat(out, name);
}

/* The file at `url`, whole, in memory (`size` bytes, mapped: the brk heap
 * is 4 MiB on riscv64, an initramfs some 20), its SHA-256 `csum`: NULL when
 * no attempt of three got it; free_fetched() unmaps it. In memory, not on
 * the ESP: nothing unchecked is written there, and the download does not
 * wait on the disk (a slow one stalled the transfer). */
static uint8_t *fetch_checked(const char *url, long long size, const char *csum) {
    size_t len = size > 0 ? (size_t)size : 1;
    uint8_t *data = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (data == MAP_FAILED) {
        say("out of memory for ", url, NULL);
        return NULL;
    }
    for (int attempt = 1; attempt <= 3; attempt++) {
        int pid = 0, fd = download_open(url, &pid);
        long long got = 0;
        ssize_t n = 0;
        while (fd >= 0 && got <= size) {
            uint8_t extra;
            n = got < size ? read(fd, data + got, (size_t)(size - got)) : read(fd, &extra, 1);
            if (n <= 0) {
                break;
            }
            got += n;
        }
        int rc = fd >= 0 ? download_close(fd, pid) : -1;
        if (rc == 0 && n == 0 && got == size) {
            sha256 s;
            char hex[65];
            sha256_init(&s);
            sha256_update(&s, data, (size_t)size);
            sha256_hex(&s, hex);
            if (strcmp(hex, csum) == 0) {
                return data;
            }
            say("checksum mismatch: ", url, NULL);
        }
        if (attempt < 3) {
            say("retrying ", url, NULL);
            sleep(2);
        }
    }
    munmap(data, len);
    return NULL;
}

static void free_fetched(uint8_t *data, long long size) {
    munmap(data, size > 0 ? (size_t)size : 1);
}

/* Download the boot files and write them into the slot of the ESP mounted
 * at `esp` (from partition /dev/<part>), then unmount, mount again and check
 * what the disk has against the list; the slot's version last. The slot's
 * old version goes first: a slot without one is never booted into by an
 * upgrade. */
static int write_slot(const char *esp, const char *part, char slot, const boot_files *b,
                      void (*url_of)(char *url, size_t cap, const char *file)) {
    char path[PATH_MAX_GV], url[512], dev[80];
    slot_path(path, esp, slot, "");
    mkdirs(path, 1);
    slot_path(path, esp, slot, "version");
    unlink(path);
    for (int i = 0; i < 2; i++) {
        slot_path(path, esp, slot, NAMES[i]);
        url_of(url, sizeof url, b->file[i]);
        say("fetching ", url, NULL);
        uint8_t *data = fetch_checked(url, b->size[i], b->csum[i]);
        if (data == NULL) {
            return die("download failed: ", url);
        }
        int rc = write_file(path, (const char *)data, (size_t)b->size[i]);
        free_fetched(data, b->size[i]);
        if (rc != 0) {
            return die("cannot write ", path);
        }
    }
    strcpy(dev, "/dev/");
    strcat(dev, part);
    if (umount_dir(esp) != 0 || mount(dev, esp, "fat") != 0) {
        return die("cannot mount the ESP again: ", dev);
    }
    for (int i = 0; i < 2; i++) {
        char hex[65];
        long long size = 0;
        slot_path(path, esp, slot, NAMES[i]);
        if (sha256_file(path, hex, &size) != 0 || size != b->size[i] || strcmp(hex, b->csum[i]) != 0) {
            return die("the disk does not hold what was downloaded: ", path);
        }
    }
    char version[260];
    copy_field(version, sizeof version - 1, b->header, strlen(b->header));
    strcat(version, "\n");
    slot_path(path, esp, slot, "version");
    if (write_file(path, version, strlen(version)) != 0) {
        return die("cannot write ", path);
    }
    return 0;
}

/* ---- limine.conf -------------------------------------------------------- */

/* Copy an entry of the config (its lines from `/name` up to the next entry)
 * with the slot `from` replaced by `to`: the entry's name, its paths and its
 * `slot=`. */
static void entry_for(char *out, size_t cap, const char *entry, char from, char to) {
    size_t n = 0;
    for (const char *p = entry; *p != '\0' && n + 1 < cap; p++) {
        char c = *p;
        int boot_dir = p >= entry + 6 && strncmp(p - 6, "/boot/", 6) == 0 && p[1] == '/';
        int slot_word = p >= entry + 5 && strncmp(p - 5, "slot=", 5) == 0;
        int name = p >= entry + 6 && strncmp(p - 6, "/myos ", 6) == 0 && (p[1] == '\n' || p[1] == '\0');
        if (c == from && (boot_dir || slot_word || name)) {
            c = to;
        }
        out[n++] = c;
    }
    out[n] = '\0';
}

/* Rewrite the config at `esp` to boot `first` by default and offer `second`
 * (0: none) in a menu, the entries made from the one of `template` (the
 * slot the config was written for); the global lines stay, but the
 * timeout. Written beside it, then renamed over it. */
static int write_conf(const char *esp, char template, char first, char second) {
    static char conf[16384], entry[4096], out[16384], one[4096];
    char path[PATH_MAX_GV], tmp[PATH_MAX_GV];
    strcpy(path, esp);
    strcat(path, "/boot/limine/limine.conf");
    int fd = open(path, O_RDONLY);
    ssize_t n = fd < 0 ? -1 : read(fd, conf, sizeof conf - 1);
    if (fd >= 0) {
        close(fd);
    }
    if (n <= 0) {
        return die("cannot read ", path);
    }
    conf[n] = '\0';
    /* The global lines: those before the first entry, timeout aside. */
    out[0] = '\0';
    entry[0] = '\0';
    const char *p = conf;
    while (*p != '\0' && *p != '/') {
        size_t len = strcspn(p, "\n");
        if (len > 0 && strncmp(p, "timeout:", 8) != 0 && strlen(out) + len + 2 < sizeof out) {
            strncat(out, p, len);
            strcat(out, "\n");
        }
        p += len + (p[len] == '\n');
    }
    /* The entry booting the template slot. */
    char want[32] = "boot():/boot/x/kernel";
    want[13] = template;
    while (*p == '/') {
        const char *end = strstr(p + 1, "\n/");
        size_t len = end != NULL ? (size_t)(end + 1 - p) : strlen(p);
        if (len < sizeof entry) {
            memcpy(entry, p, len);
            /* One newline at its end, whatever blank lines followed. */
            size_t keep = len;
            while (keep > 1 && entry[keep - 1] == '\n' && entry[keep - 2] == '\n') {
                keep--;
            }
            entry[keep] = '\0';
            if (strstr(entry, want) != NULL) {
                break;
            }
        }
        entry[0] = '\0';
        p += len;
    }
    if (entry[0] == '\0') {
        return die("limine.conf has no entry for slot ", (char[]){template, '\0'});
    }
    if (strstr(entry, "slot=") == NULL) {
        return die("limine.conf's entry does not say its slot (cmdline: slot=)", NULL);
    }
    /* `timeout:` first: with a fallback, a short menu to pick it. */
    char head[32];
    strcpy(head, second != 0 ? "timeout: 3\n" : "timeout: 0\n");
    char body[16384];
    strcpy(body, head);
    strncat(body, out, sizeof body - strlen(body) - 1);
    for (int i = 0; i < 2; i++) {
        char slot = i == 0 ? first : second;
        if (slot == 0) {
            continue;
        }
        entry_for(one, sizeof one, entry, template, slot);
        if (strlen(body) + strlen(one) + 2 >= sizeof body) {
            return die("limine.conf too long", NULL);
        }
        /* A blank line before each entry, as the images write them. */
        strcat(body, "\n");
        strcat(body, one);
    }
    strcpy(tmp, path);
    strcat(tmp, ".new");
    if (write_file(tmp, body, strlen(body)) != 0 || rename(tmp, path) != 0) {
        unlink(tmp);
        return die("cannot write ", path);
    }
    return 0;
}

/* ---- --upgrade ---------------------------------------------------------- */

int boot_upgrade(const char *list_path, void (*url_of)(char *url, size_t cap, const char *file), int force) {
    boot_files b;
    if (fetch_boot_list(list_path, &b) != 0) {
        return die("the mirror's boot file list is bad or missing", NULL);
    }
    char slot = running_slot();
    if (slot == 0) {
        return die("this boot does not say its slot (/proc/cmdline has no slot=): "
                   "the disk predates the boot slots (docs/install.md)", NULL);
    }
    char other = slot == 'a' ? 'b' : 'a';
    char part[64], path[PATH_MAX_GV], have[260], rel[32];
    if (mount_running_esp(slot, part, sizeof part) != 0) {
        return 1;
    }
    slot_path(path, RUNNING_ESP, slot, "version");
    read_line(path, have, sizeof have);
    word_value(have, "release", rel, sizeof rel);
    say("running slot ", (char[]){slot, '\0'}, NULL);
    say("  ", have, NULL);
    say("mirror: ", b.header, NULL);
    if (!force && atoll(b.release) <= atoll(rel)) {
        umount_dir(RUNNING_ESP);
        say("up to date (-f writes the other slot anyway)", NULL, NULL);
        return 0;
    }
    int rc = write_slot(RUNNING_ESP, part, other, &b, url_of);
    if (rc == 0) {
        rc = write_conf(RUNNING_ESP, slot, other, slot);
    }
    if (umount_dir(RUNNING_ESP) != 0 && rc == 0) {
        rc = die("cannot unmount the ESP", NULL);
    }
    if (rc != 0) {
        return die("the upgrade stopped: the running slot and limine.conf are as they were", NULL);
    }
    say("slot ", (char[]){other, '\0'}, " has the new release and boots by default: reboot to start it");
    say("(the boot menu offers slot ", (char[]){slot, '\0'}, " for 3 seconds as the fallback)");
    return 0;
}

/* ---- --install ---------------------------------------------------------- */

static uint32_t crc32(const uint8_t *p, size_t n) {
    uint32_t crc = 0xFFFFFFFFu;
    while (n--) {
        crc ^= *p++;
        for (int k = 0; k < 8; k++) {
            crc = (crc >> 1) ^ (0xEDB88320u & (uint32_t)-(int32_t)(crc & 1));
        }
    }
    return ~crc;
}

static void put32(uint8_t *p, uint32_t v) {
    for (int i = 0; i < 4; i++) {
        p[i] = (uint8_t)(v >> (8 * i));
    }
}

static void put64(uint8_t *p, uint64_t v) {
    for (int i = 0; i < 8; i++) {
        p[i] = (uint8_t)(v >> (8 * i));
    }
}

/* A random version-4 GUID. */
static void random_guid(uint8_t g[16]) {
    int fd = open("/dev/urandom", O_RDONLY);
    if (fd < 0 || read(fd, g, 16) != 16) {
        memset(g, 0x5a, 16);
    }
    if (fd >= 0) {
        close(fd);
    }
    g[7] = (uint8_t)((g[7] & 0x0F) | 0x40);
    g[8] = (uint8_t)((g[8] & 0x3F) | 0x80);
}

static const uint8_t BIOS_BOOT_TYPE[16] = {0x48, 0x61, 0x68, 0x21, 0x49, 0x64, 0x6F, 0x6E,
                                           0x74, 0x4E, 0x65, 0x65, 0x64, 0x45, 0x46, 0x49};
static const uint8_t ESP_TYPE_BYTES[16] = {0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11,
                                           0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B};
static const uint8_t LINUX_DATA_TYPE[16] = {0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47,
                                            0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D, 0xE4};

static void gpt_entry(uint8_t *e, const uint8_t type[16], uint64_t first, uint64_t last, uint64_t attrs,
                      const char *name) {
    memcpy(e, type, 16);
    random_guid(e + 16);
    put64(e + 32, first);
    put64(e + 40, last);
    put64(e + 48, attrs);
    for (size_t i = 0; name[i] != '\0' && i < 36; i++) {
        e[56 + 2 * i] = (uint8_t)name[i];
    }
}

static void gpt_header(uint8_t *h, uint64_t this_lba, uint64_t alt, uint64_t entries_lba, uint64_t total,
                       const uint8_t disk_guid[16], uint32_t entries_crc) {
    memset(h, 0, SECTOR);
    memcpy(h, "EFI PART", 8);
    put32(h + 8, 0x00010000);
    put32(h + 12, 92);
    put64(h + 24, this_lba);
    put64(h + 32, alt);
    put64(h + 40, 34);
    put64(h + 48, total - 34);
    memcpy(h + 56, disk_guid, 16);
    put64(h + 72, entries_lba);
    put32(h + 80, 128);
    put32(h + 84, 128);
    put32(h + 88, entries_crc);
    put32(h + 16, crc32(h, 92));
}

static int pwrite_all(int fd, const uint8_t *p, size_t n, uint64_t at) {
    if (lseek(fd, (off_t)at, SEEK_SET) == (off_t)-1) {
        return -1;
    }
    while (n > 0) {
        ssize_t w = write(fd, p, n);
        if (w <= 0) {
            return -1;
        }
        p += w, n -= (size_t)w;
    }
    return 0;
}

/* The GPT of the boot disk on the device open at fd, `total` sectors. */
static int write_gpt(int fd, uint64_t total) {
    static uint8_t entries[128 * 128], head[34 * SECTOR], tail[33 * SECTOR];
    uint64_t esp_first = 2 * MIB, esp_last = esp_first + 512 * MIB - 1;
    /* The data partition to the last usable sector, ending on a MiB. */
    uint64_t data_first = esp_last + 1;
    uint64_t data_last = (total - 34) / MIB * MIB - 1;
    memset(entries, 0, sizeof entries);
    gpt_entry(entries, BIOS_BOOT_TYPE, MIB, 2 * MIB - 1, 4, "BIOS Boot");
    gpt_entry(entries + 128, ESP_TYPE_BYTES, esp_first, esp_last, 1, "EFI System");
    gpt_entry(entries + 256, LINUX_DATA_TYPE, data_first, data_last, 0, "myos data");
    uint32_t crc = crc32(entries, sizeof entries);
    uint8_t disk_guid[16];
    random_guid(disk_guid);
    memset(head, 0, sizeof head);
    /* The protective MBR: one partition of type 0xEE over the disk. */
    uint8_t *mbr = head + 446;
    mbr[2] = 0x02;
    mbr[4] = 0xEE;
    mbr[5] = mbr[6] = mbr[7] = 0xFF;
    put32(mbr + 8, 1);
    put32(mbr + 12, total - 1 > 0xFFFFFFFFu ? 0xFFFFFFFFu : (uint32_t)(total - 1));
    head[510] = 0x55;
    head[511] = 0xAA;
    gpt_header(head + SECTOR, 1, total - 1, 2, total, disk_guid, crc);
    memcpy(head + 2 * SECTOR, entries, sizeof entries);
    memcpy(tail, entries, sizeof entries);
    gpt_header(tail + 32 * SECTOR, total - 1, 1, total - 33, total, disk_guid, crc);
    if (pwrite_all(fd, head, sizeof head, 0) != 0 || pwrite_all(fd, tail, sizeof tail, (total - 33) * SECTOR) != 0) {
        return -1;
    }
    return fsync(fd);
}

/* Copy a file of the running ESP to the same path on the new one. */
static int copy_esp_file(const char *rel) {
    char from[PATH_MAX_GV], to[PATH_MAX_GV];
    static uint8_t buf[65536];
    strcpy(from, RUNNING_ESP "/");
    strcat(from, rel);
    strcpy(to, NEW_ESP "/");
    strcat(to, rel);
    int in = open(from, O_RDONLY);
    if (in < 0) {
        return -1;
    }
    mkdirs(to, 0);
    int out = open(to, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    ssize_t n = 0;
    while (out >= 0 && (n = read(in, buf, sizeof buf)) > 0) {
        if (write(out, buf, (size_t)n) != n) {
            n = -1;
            break;
        }
    }
    close(in);
    if (out < 0 || close(out) != 0 || n < 0) {
        return die("cannot copy ", rel);
    }
    return 0;
}

/* Limine and what its config names beside the slots: every file of
 * EFI/BOOT and boot/limine (but the config, written for the slots), the
 * files directly in boot/ (the device trees of aarch64 and riscv64) and
 * startup.nsh. */
static int copy_limine(void) {
    const char *dirs[] = {"EFI/BOOT", "boot/limine", "boot", ""};
    for (size_t i = 0; i < sizeof dirs / sizeof *dirs; i++) {
        char dir[PATH_MAX_GV];
        strcpy(dir, RUNNING_ESP "/");
        strcat(dir, dirs[i]);
        DIR *d = opendir(dir);
        if (d == NULL) {
            continue;
        }
        struct dirent *e;
        int rc = 0;
        while (rc == 0 && (e = readdir(d)) != NULL) {
            char rel[PATH_MAX_GV], full[PATH_MAX_GV];
            struct stat st;
            if (e->d_name[0] == '.' || strcmp(e->d_name, "limine.conf") == 0) {
                continue;
            }
            if (dirs[i][0] == '\0' && strcmp(e->d_name, "startup.nsh") != 0) {
                continue;
            }
            strcpy(rel, dirs[i]);
            if (rel[0] != '\0') {
                strcat(rel, "/");
            }
            strncat(rel, e->d_name, sizeof rel - strlen(rel) - 1);
            strcpy(full, RUNNING_ESP "/");
            strncat(full, rel, sizeof full - strlen(full) - 1);
            if (stat(full, &st) == 0 && S_ISREG(st.st_mode)) {
                rc = copy_esp_file(rel);
            }
        }
        closedir(d);
        if (rc != 0) {
            return rc;
        }
    }
    return 0;
}

/* The disk's size in sectors: the first one a read gets nothing from.
 * Not lseek(SEEK_END): libgloss's lseek returns an int, which riscv64's
 * calling convention cuts to 32 bits (issue #366). */
static uint64_t disk_sectors(int fd) {
    uint8_t sec[SECTOR];
    uint64_t lo = 0, hi = 1;
    /* lo sectors are readable; find a hi that is not, then halve. */
    for (;;) {
        if (lseek(fd, (off_t)((hi - 1) * SECTOR), SEEK_SET) == (off_t)-1 || read(fd, sec, SECTOR) != SECTOR) {
            break;
        }
        lo = hi;
        if (hi >= (1ULL << 40)) {
            return lo;
        }
        hi *= 2;
    }
    while (hi - lo > 1) {
        uint64_t mid = lo + (hi - lo) / 2;
        if (lseek(fd, (off_t)((mid - 1) * SECTOR), SEEK_SET) != (off_t)-1 && read(fd, sec, SECTOR) == SECTOR) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    return lo;
}

/* A partition of the disk is mounted (/proc/mounts names /dev/<disk>/...). */
static int disk_mounted(const char *disk) {
    char line[512], prefix[80];
    strcpy(prefix, "/dev/");
    strcat(prefix, disk);
    strcat(prefix, "/");
    FILE *f = fopen("/proc/mounts", "r");
    if (f == NULL) {
        return 0;
    }
    int found = 0;
    while (!found && fgets(line, sizeof line, f) != NULL) {
        found = strncmp(line, prefix, strlen(prefix)) == 0;
    }
    fclose(f);
    return found;
}

int boot_install(const char *disk_arg, const char *list_path,
                 void (*url_of)(char *url, size_t cap, const char *file)) {
    boot_files b;
    char disk[64], data[96], esp_dev[96], part_dev[96], esp_part[96], running[64];
    /* DISK: "nvme1n1", "/dev/nvme1n1" or "/dev/nvme1n1/data". */
    const char *d = disk_arg;
    if (strncmp(d, "/dev/", 5) == 0) {
        d += 5;
    }
    copy_field(disk, sizeof disk, d, strcspn(d, "/"));
    if (disk[0] == '\0') {
        return die("no disk named", NULL);
    }
    strcpy(data, "/dev/");
    strcat(data, disk);
    strcat(data, "/data");
    if (fetch_boot_list(list_path, &b) != 0) {
        return die("the mirror's boot file list is bad or missing", NULL);
    }
    char slot = running_slot();
    if (slot == 0) {
        return die("this boot does not say its slot (/proc/cmdline has no slot=): "
                   "Limine is copied from the running boot disk's ESP, which needs it", NULL);
    }
    if (mount_running_esp(slot, running, sizeof running) != 0) {
        return 1;
    }
    size_t rl = strcspn(running, "/");
    if (strlen(disk) == rl && strncmp(running, disk, rl) == 0) {
        umount_dir(RUNNING_ESP);
        return die("refusing to install over the running boot disk: ", disk);
    }
    if (disk_mounted(disk)) {
        umount_dir(RUNNING_ESP);
        return die("a partition of the disk is mounted (/proc/mounts): ", disk);
    }
    int fd = open(data, O_RDWR);
    if (fd < 0) {
        umount_dir(RUNNING_ESP);
        return die("no such disk: ", data);
    }
    uint64_t total = disk_sectors(fd);
    /* The ESP and some room for the data partition. */
    if (total < 2 * MIB + 512 * MIB + 64 * MIB + 34) {
        close(fd);
        umount_dir(RUNNING_ESP);
        return die("the disk is too small (580 MiB at least): ", disk);
    }
    say("writing the boot disk layout on ", data, NULL);
    int rc = write_gpt(fd, total);
    close(fd);
    if (rc != 0) {
        umount_dir(RUNNING_ESP);
        return die("cannot write the partition table on ", data);
    }
    /* The kernel reads the new table (its old partitions are not in use). */
    write_file("/proc/pci", "rescan\n", 7);
    strcpy(esp_dev, "/dev/");
    strcat(esp_dev, disk);
    strcat(esp_dev, "/p2");
    strcpy(part_dev, "/dev/");
    strcat(part_dev, disk);
    strcat(part_dev, "/p3");
    char *mkfat[] = {"mkfs.fat", esp_dev, NULL};
    char *mkext2[] = {"mkfs.ext2", part_dev, NULL};
    if (run(mkfat) != 0 || run(mkext2) != 0) {
        umount_dir(RUNNING_ESP);
        return die("cannot format the partitions of ", disk);
    }
    mkdirs(NEW_ESP, 1);
    if (mount(esp_dev, NEW_ESP, "fat") != 0) {
        umount_dir(RUNNING_ESP);
        return die("cannot mount the new ESP ", esp_dev);
    }
    rc = copy_limine();
    if (rc == 0) {
        char dir[PATH_MAX_GV];
        slot_path(dir, NEW_ESP, 'b', "");
        mkdirs(dir, 1);
        /* The config: the running one's global lines and its entry, for
         * slot a alone (written before the slot, rewritten after). */
        rc = copy_esp_file("boot/limine/limine.conf");
        copy_field(esp_part, sizeof esp_part, esp_dev + 5, strlen(esp_dev + 5));
        if (rc == 0) {
            rc = write_slot(NEW_ESP, esp_part, 'a', &b, url_of);
        }
        if (rc == 0) {
            /* write_conf reads NEW_ESP's copy of the running config. */
            rc = write_conf(NEW_ESP, slot, 'a', 0);
        }
    }
    umount_dir(NEW_ESP);
    umount_dir(RUNNING_ESP);
    if (rc != 0) {
        return die("the install stopped; the disk is not bootable: ", disk);
    }
    say("installed on ", disk, ": slot a has the mirror's release (UEFI boot; no BIOS stage yet)");
    return 0;
}
