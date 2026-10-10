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
 * --upgrade also brings Limine's files on the ESP to the release's (the
 * `esp` lines of the list) when they differ: each new one is written beside
 * the old (`.new`), read back after a remount and renamed over it; on x86 a
 * new limine-bios.sys reruns `limine bios-install` for the BIOS stage that
 * goes with it, and limine.conf's global lines then come from the
 * release's config, which the new Limine reads.
 *
 * --install DISK lays the boot disk out on DISK (the running boot disk, or
 * one with a mounted partition, is refused): a GPT with a BIOS boot
 * partition (1 MiB), the ESP (512 MiB, mkfs.fat) and the data partition
 * (the rest, mkfs.ext2); writes Limine's files from the release (the `esp`
 * lines of the list: the EFI binary, the device tree, the config booting
 * slot a) and fstab (/boot/fstab once booted), which names the data
 * partition for `mount -a` to mount at /data; fills slot a, and on x86
 * runs `limine bios-install` (the tool's port, ports/limine) for the BIOS
 * stage. Nothing comes from the running system's ESP, so a system booted
 * from the ISO installs the same.
 *
 * The running ESP is used where it is mounted (/boot, by `mount -a` at
 * boot), else mounted at RUNNING_ESP for the while.
 *
 * --install DISK --local needs no mirror: the list names files of the
 * running system instead (boot_local_list): the kernel and the initramfs
 * this boot came from (/proc/boot/), Limine's files from the initramfs
 * (/lib/myos-boot/). A file of a list whose name starts with `/` is read
 * from there, any other is downloaded from the mirror.
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

static void slot_path(char *out, const char *esp, char slot, const char *name);

/* Where the running boot disk's ESP is mounted: /boot (`mount -a` mounts it
 * there at boot), else RUNNING_ESP, mounted by mount_running_esp and
 * unmounted by release_esp. */
static char esp_dir[96];
static int esp_premounted;

/* The mount point of /dev/<part> (/proc/mounts) into out[cap]; -1 when it
 * is not mounted. */
static int mounted_dir(const char *part, char *out, size_t cap) {
    char line[512], dev[80];
    FILE *f = fopen("/proc/mounts", "r");
    if (f == NULL) {
        return -1;
    }
    strcpy(dev, "/dev/");
    strcat(dev, part);
    strcat(dev, " ");
    int rc = -1;
    while (rc != 0 && fgets(line, sizeof line, f) != NULL) {
        if (strncmp(line, dev, strlen(dev)) == 0) {
            const char *t = line + strlen(dev);
            copy_field(out, cap, t, strcspn(t, " \n"));
            rc = 0;
        }
    }
    fclose(f);
    return rc;
}

/* Unmount the running ESP if mount_running_esp mounted it. */
static int release_esp(void) {
    return esp_premounted ? 0 : umount_dir(esp_dir);
}

/* The running boot disk's ESP at esp_dir: of the ESP partitions
 * (/proc/partitions), the one whose running slot has this system's release
 * (several disks may carry an ESP), where it is mounted already (/boot) or
 * mounted at RUNNING_ESP. Its partition name ("nvme2n1/p2") into name[cap]. */
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
        esp_premounted = mounted_dir(part, esp_dir, sizeof esp_dir) == 0;
        if (!esp_premounted) {
            strcpy(esp_dir, RUNNING_ESP);
            if (mount(dev, RUNNING_ESP, "fat") != 0) {
                continue;
            }
        }
        if (strlen(esp_dir) + 20 >= sizeof path) {
            release_esp();
            continue;
        }
        slot_path(path, esp_dir, slot, "version");
        if (read_line(path, version, sizeof version) == 0 && strcmp(version, release) == 0) {
            copy_field(name, cap, part, strlen(part));
            found = 0;
        } else {
            release_esp();
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

/* Limine's files for an ESP --install makes: at most this many. */
#define MAX_ESP 8

typedef struct {
    char path[96], csum[72], file[128];
    long long size;
} esp_file;

typedef struct {
    char header[256];  /* "release=... commit=... abi=..." */
    char release[32];
    char csum[2][72], file[2][96];
    long long size[2];
    esp_file esp[MAX_ESP];
    int nesp;
} boot_files;

/* <arch>-boot.txt: the header, then "kernel SIZE SHA256 FILE" and the same
 * for the initramfs, and "esp PATH SIZE SHA256 FILE" for each of Limine's
 * files on the ESP. */
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
        if (strncmp(line, "esp ", 4) == 0 && b->nesp < MAX_ESP) {
            esp_file *e = &b->esp[b->nesp];
            char *p = line + 4, *sp;
            copy_field(e->path, sizeof e->path, p, strcspn(p, " "));
            if ((sp = strchr(p, ' ')) == NULL) {
                continue;
            }
            e->size = atoll(sp + 1);
            if ((sp = strchr(sp + 1, ' ')) == NULL) {
                continue;
            }
            copy_field(e->csum, sizeof e->csum, sp + 1, strcspn(sp + 1, " "));
            if ((sp = strchr(sp + 1, ' ')) == NULL) {
                continue;
            }
            copy_field(e->file, sizeof e->file, sp + 1, strlen(sp + 1));
            /* A path stays on the ESP: no `..`, no absolute one. */
            if (e->path[0] != '/' && strstr(e->path, "..") == NULL) {
                b->nesp++;
            }
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

static const char *NAMES[2] = {"kernel", "initramfs"};

/* --install --local: the running system's boot files as a list at `path`,
 * in the mirror's format, with absolute paths: the release of
 * /lib/myos-release, the kernel and the initramfs this boot came from
 * (/proc/boot/, their checksums taken here), and Limine's files the
 * initramfs carries (/lib/myos-boot/boot.txt, the `esp` lines). */
int boot_local_list(const char *path) {
    static const char *FILES[2] = {"/proc/boot/kernel", "/proc/boot/initramfs"};
    char release[256], line[512], hex[65], size[24];
    if (read_line("/lib/myos-release", release, sizeof release) != 0) {
        return die("no /lib/myos-release", NULL);
    }
    FILE *out = fopen(path, "w");
    FILE *esp = fopen("/lib/myos-boot/boot.txt", "r");
    if (out == NULL || esp == NULL) {
        if (out != NULL) {
            fclose(out);
        }
        if (esp != NULL) {
            fclose(esp);
        }
        return die("no Limine files in this system (/lib/myos-boot/boot.txt)", NULL);
    }
    fputs("# myos ", out);
    fputs(release, out);
    fputs("\n", out);
    int rc = 0;
    for (int i = 0; i < 2; i++) {
        long long n = 0;
        if (sha256_file(FILES[i], hex, &n) != 0 || n <= 0) {
            rc = die("cannot read ", FILES[i]);
            break;
        }
        char *p = size + sizeof size - 1;
        *p = '\0';
        do {
            *--p = (char)('0' + n % 10);
            n /= 10;
        } while (n > 0);
        fputs(NAMES[i], out);
        fputs(" ", out);
        fputs(p, out);
        fputs(" ", out);
        fputs(hex, out);
        fputs(" ", out);
        fputs(FILES[i], out);
        fputs("\n", out);
    }
    while (rc == 0 && fgets(line, sizeof line, esp) != NULL) {
        fputs(line, out);
    }
    fclose(esp);
    if (fclose(out) != 0 && rc == 0) {
        rc = die("cannot write ", path);
    }
    return rc;
}

/* ---- writing a slot ----------------------------------------------------- */

/* ESP-relative path of a slot's file into out: "<esp>/boot/<slot>/<name>". */
static void slot_path(char *out, const char *esp, char slot, const char *name) {
    strcpy(out, esp);
    strcat(out, "/boot/x/");
    out[strlen(esp) + 6] = slot;
    strcat(out, name);
}

/* The file `file` of a list, whole, in memory (`size` bytes, mapped: the
 * brk heap is 4 MiB on riscv64, an initramfs some 20), its SHA-256 `csum`:
 * NULL when no attempt of three got it; free_fetched() unmaps it. A file
 * named by an absolute path is the running system's (--local), read from
 * there; any other is downloaded from the mirror. In memory, not on the
 * ESP: nothing unchecked is written there, and the download does not wait
 * on the disk (a slow one stalled the transfer). */
static uint8_t *fetch_checked(const char *file, long long size, const char *csum,
                              void (*url_of)(char *url, size_t cap, const char *file)) {
    char url[512];
    int local = file[0] == '/';
    if (local) {
        copy_field(url, sizeof url, file, strlen(file));
    } else {
        url_of(url, sizeof url, file);
    }
    say(local ? "reading " : "fetching ", url, NULL);
    size_t len = size > 0 ? (size_t)size : 1;
    uint8_t *data = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (data == MAP_FAILED) {
        say("out of memory for ", url, NULL);
        return NULL;
    }
    for (int attempt = 1; attempt <= 3; attempt++) {
        int pid = 0, fd = local ? open(url, O_RDONLY) : download_open(url, &pid);
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
        int rc = fd < 0 ? -1 : local ? close(fd) : download_close(fd, pid);
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

/* Unmount the ESP at `esp` and mount its partition /dev/<part> there again:
 * what is read after comes from the disk. */
static int remount_esp(const char *esp, const char *part) {
    char dev[80];
    strcpy(dev, "/dev/");
    strcat(dev, part);
    if (umount_dir(esp) != 0 || mount(dev, esp, "fat") != 0) {
        return die("cannot mount the ESP again: ", dev);
    }
    return 0;
}

/* The file at `path` has `size` bytes of SHA-256 `csum`. */
static int file_is(const char *path, long long size, const char *csum) {
    char hex[65];
    long long got = 0;
    return sha256_file(path, hex, &got) == 0 && got == size && strcmp(hex, csum) == 0;
}

/* "<esp>/<rel><suffix>" into out[PATH_MAX_GV]. */
static int esp_path(char *out, const char *esp, const char *rel, const char *suffix) {
    if (strlen(esp) + 1 + strlen(rel) + strlen(suffix) >= PATH_MAX_GV) {
        return die("path too long: ", rel);
    }
    strcpy(out, esp);
    strcat(out, "/");
    strcat(out, rel);
    strcat(out, suffix);
    return 0;
}

/* Get the boot files and write them into the slot of the ESP mounted at
 * `esp` (from partition /dev/<part>), then unmount, mount again and check
 * what the disk has against the list; the slot's version last. The slot's
 * old version goes first: a slot without one is never booted into by an
 * upgrade. */
static int write_slot(const char *esp, const char *part, char slot, const boot_files *b,
                      void (*url_of)(char *url, size_t cap, const char *file)) {
    char path[PATH_MAX_GV];
    slot_path(path, esp, slot, "");
    mkdirs(path, 1);
    slot_path(path, esp, slot, "version");
    unlink(path);
    for (int i = 0; i < 2; i++) {
        slot_path(path, esp, slot, NAMES[i]);
        uint8_t *data = fetch_checked(b->file[i], b->size[i], b->csum[i], url_of);
        if (data == NULL) {
            return die("cannot get ", b->file[i]);
        }
        int rc = write_file(path, (const char *)data, (size_t)b->size[i]);
        free_fetched(data, b->size[i]);
        if (rc != 0) {
            return die("cannot write ", path);
        }
    }
    if (remount_esp(esp, part) != 0) {
        return 1;
    }
    for (int i = 0; i < 2; i++) {
        slot_path(path, esp, slot, NAMES[i]);
        if (!file_is(path, b->size[i], b->csum[i])) {
            return die("the disk does not hold what the list has: ", path);
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

/* The config at `esp` into conf[cap], NUL-terminated. */
static int read_conf(const char *esp, char *conf, size_t cap) {
    char path[PATH_MAX_GV];
    strcpy(path, esp);
    strcat(path, "/boot/limine/limine.conf");
    int fd = open(path, O_RDONLY);
    ssize_t n = fd < 0 ? -1 : read(fd, conf, cap - 1);
    if (fd >= 0) {
        close(fd);
    }
    if (n <= 0) {
        return die("cannot read ", path);
    }
    conf[n] = '\0';
    return 0;
}

/* Rewrite the config at `esp` to boot `first` by default and offer `second`
 * (0: none) in a menu, from `conf` (the running config, or the release's
 * when Limine changed): its global lines, but the timeout, and the
 * entries made from its one of `template` (the slot it was written for).
 * Written beside the old, then renamed over it. */
static int write_conf(const char *esp, const char *conf, char template, char first, char second) {
    static char entry[4096], out[16384], one[4096];
    char path[PATH_MAX_GV], tmp[PATH_MAX_GV];
    strcpy(path, esp);
    strcat(path, "/boot/limine/limine.conf");
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

#define LIMINE_CONF "boot/limine/limine.conf"

#if defined(__x86_64__)
/* The BIOS stage: Limine's MBR code and its stage 2 in the BIOS boot
 * partition (GPT entry 1) of /dev/<disk>/data, as the host does for the
 * images; the one that goes with the ESP's limine-bios.sys. */
static int bios_install(const char *disk) {
    char data[96];
    strcpy(data, "/dev/");
    strcat(data, disk);
    strcat(data, "/data");
    char *argv[] = {"limine", "bios-install", data, "1", NULL};
    if (run(argv) != 0) {
        return die("limine bios-install failed on ", data);
    }
    return 0;
}
#endif

/* Bring Limine's files on the running ESP (mounted at `esp` from /dev/<part>)
 * to the release's, those that differ (limine.conf aside: write_conf makes
 * it): each new one beside the old (`.new`), all of them read back after a
 * remount, then renamed over the old ones; a new limine-bios.sys reruns
 * `limine bios-install` (x86). *changed counts the replaced files. A
 * failure before the renames leaves the old files in place. */
static int refresh_limine(const char *esp, const char *part, const boot_files *b,
                          void (*url_of)(char *url, size_t cap, const char *file), int *changed) {
    int stale[MAX_ESP] = {0}, n = 0;
    char path[PATH_MAX_GV], tmp[PATH_MAX_GV];
    *changed = 0;
    for (int i = 0; i < b->nesp; i++) {
        const esp_file *e = &b->esp[i];
        if (strcmp(e->path, LIMINE_CONF) == 0 || esp_path(path, esp, e->path, "") != 0
            || file_is(path, e->size, e->csum)) {
            continue;
        }
        uint8_t *data = fetch_checked(e->file, e->size, e->csum, url_of);
        if (data == NULL) {
            return die("cannot get ", e->file);
        }
        esp_path(tmp, esp, e->path, ".new");
        mkdirs(tmp, 0);
        int rc = write_file(tmp, (const char *)data, (size_t)e->size);
        free_fetched(data, e->size);
        if (rc != 0) {
            return die("cannot write ", tmp);
        }
        stale[i] = 1;
        n++;
    }
    if (n == 0) {
        return 0;
    }
    if (remount_esp(esp, part) != 0) {
        return 1;
    }
    for (int i = 0; i < b->nesp; i++) {
        esp_path(tmp, esp, b->esp[i].path, ".new");
        if (stale[i] && !file_is(tmp, b->esp[i].size, b->esp[i].csum)) {
            return die("the disk does not hold what the list has: ", tmp);
        }
    }
    int bios = 0;
    for (int i = 0; i < b->nesp; i++) {
        if (!stale[i]) {
            continue;
        }
        esp_path(path, esp, b->esp[i].path, "");
        esp_path(tmp, esp, b->esp[i].path, ".new");
        if (rename(tmp, path) != 0) {
            return die("cannot replace ", path);
        }
        say("Limine: replaced ", b->esp[i].path, NULL);
        bios |= strcmp(b->esp[i].path, "boot/limine/limine-bios.sys") == 0;
        (*changed)++;
    }
#if defined(__x86_64__)
    if (bios) {
        char disk[64];
        copy_field(disk, sizeof disk, part, strcspn(part, "/"));
        if (bios_install(disk) != 0) {
            return 1;
        }
        say("Limine: rewrote the BIOS stage on ", disk, NULL);
    }
#else
    (void)bios;
#endif
    return 0;
}

/* The release's limine.conf (its `esp` line) into conf[cap]. */
static int release_conf(const boot_files *b, void (*url_of)(char *url, size_t cap, const char *file),
                        char *conf, size_t cap) {
    for (int i = 0; i < b->nesp; i++) {
        const esp_file *e = &b->esp[i];
        if (strcmp(e->path, LIMINE_CONF) != 0) {
            continue;
        }
        if (e->size <= 0 || (size_t)e->size >= cap) {
            return die("the release's limine.conf is too big", NULL);
        }
        uint8_t *data = fetch_checked(e->file, e->size, e->csum, url_of);
        if (data == NULL) {
            return die("cannot get ", e->file);
        }
        memcpy(conf, data, (size_t)e->size);
        conf[e->size] = '\0';
        free_fetched(data, e->size);
        return 0;
    }
    return die("the boot list has no limine.conf (esp line)", NULL);
}

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
    slot_path(path, esp_dir, slot, "version");
    read_line(path, have, sizeof have);
    word_value(have, "release", rel, sizeof rel);
    say("running slot ", (char[]){slot, '\0'}, NULL);
    say("  ", have, NULL);
    say("mirror: ", b.header, NULL);
    if (!force && atoll(b.release) <= atoll(rel)) {
        release_esp();
        say("up to date (-f writes the other slot anyway)", NULL, NULL);
        return 0;
    }
    /* The slot, then Limine, then the config: a new Limine reads the
     * release's config, whose entry is slot a's. */
    static char conf[16384];
    int changed = 0;
    int rc = write_slot(esp_dir, part, other, &b, url_of);
    if (rc == 0) {
        rc = refresh_limine(esp_dir, part, &b, url_of, &changed);
    }
    if (rc == 0) {
        rc = changed > 0 ? release_conf(&b, url_of, conf, sizeof conf) : read_conf(esp_dir, conf, sizeof conf);
    }
    if (rc == 0) {
        rc = write_conf(esp_dir, conf, changed > 0 ? 'a' : slot, other, slot);
    }
    if (release_esp() != 0 && rc == 0) {
        rc = die("cannot unmount the ESP", NULL);
    }
    if (rc != 0) {
        return die(changed > 0 ? "the upgrade stopped after Limine was replaced: the running slot and "
                                 "limine.conf are as they were"
                               : "the upgrade stopped: the running slot and limine.conf are as they were",
                   NULL);
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

/* The GPT of the boot disk on the device open at fd, `total` sectors; the
 * data partition's unique GUID into data_guid. */
static int write_gpt(int fd, uint64_t total, uint8_t data_guid[16]) {
    static uint8_t entries[128 * 128], head[34 * SECTOR], tail[33 * SECTOR];
    uint64_t esp_first = 2 * MIB, esp_last = esp_first + 512 * MIB - 1;
    /* The data partition to the last usable sector, ending on a MiB. */
    uint64_t data_first = esp_last + 1;
    uint64_t data_last = (total - 34) / MIB * MIB - 1;
    memset(entries, 0, sizeof entries);
    gpt_entry(entries, BIOS_BOOT_TYPE, MIB, 2 * MIB - 1, 4, "BIOS Boot");
    gpt_entry(entries + 128, ESP_TYPE_BYTES, esp_first, esp_last, 1, "EFI System");
    gpt_entry(entries + 256, LINUX_DATA_TYPE, data_first, data_last, 0, "myos data");
    memcpy(data_guid, entries + 256 + 16, 16);
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
    /* The BIOS boot partition starts empty: `limine bios-install` refuses
     * one that holds what looks like a filesystem (the disk's old one). */
    static uint8_t zero[64 * 1024];
    for (uint64_t at = MIB * SECTOR; at < 2 * MIB * SECTOR; at += sizeof zero) {
        if (pwrite_all(fd, zero, sizeof zero, at) != 0) {
            return -1;
        }
    }
    return fsync(fd);
}

/* Limine's files from the release (`esp` lines of the list) onto the ESP
 * mounted at `esp`, each checked before it is written: the EFI binary,
 * x86's limine-bios.sys, the device tree, and the config, which boots
 * slot a. */
static int write_limine(const char *esp, const boot_files *b,
                        void (*url_of)(char *url, size_t cap, const char *file)) {
    if (b->nesp == 0) {
        return die("the mirror's boot list has no Limine files (esp lines)", NULL);
    }
    for (int i = 0; i < b->nesp; i++) {
        const esp_file *e = &b->esp[i];
        char path[PATH_MAX_GV];
        if (esp_path(path, esp, e->path, "") != 0) {
            return 1;
        }
        uint8_t *data = fetch_checked(e->file, e->size, e->csum, url_of);
        if (data == NULL) {
            return die("cannot get ", e->file);
        }
        mkdirs(path, 0);
        int rc = write_file(path, (const char *)data, (size_t)e->size);
        free_fetched(data, e->size);
        if (rc != 0) {
            return die("cannot write ", path);
        }
    }
    return 0;
}

/* The ESP's fstab (at `esp`; /boot/fstab once booted): the data partition,
 * of unique GUID `g`, at /data, for `mount -a` at boot; as the host writes
 * it for the images (src/limine_disk.rs). */
static int write_fstab(const char *esp, const uint8_t g[16]) {
    static const char HEX[] = "0123456789abcdef";
    /* The GUID's text: its first three fields little-endian. */
    static const int ORDER[16] = {3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15};
    char guid[37], path[PATH_MAX_GV], text[512];
    size_t n = 0;
    for (int i = 0; i < 16; i++) {
        if (i == 4 || i == 6 || i == 8 || i == 10) {
            guid[n++] = '-';
        }
        guid[n++] = HEX[g[ORDER[i]] >> 4];
        guid[n++] = HEX[g[ORDER[i]] & 15];
    }
    guid[n] = '\0';
    strcpy(text, "# What `mount -a` mounts at boot (docs/install.md): PARTUUID=<guid> MOUNTPOINT FSTYPE [rw],\n"
                 "# the partition's unique GUID from /proc/partitions; a mount point under /mnt is made.\n"
                 "PARTUUID=");
    strcat(text, guid);
    strcat(text, " /data ext2\n");
    if (esp_path(path, esp, "fstab", "") != 0 || write_file(path, text, strlen(text)) != 0) {
        return die("cannot write the ESP's fstab", NULL);
    }
    return 0;
}

/* The disk's size in sectors (0 when it cannot tell). */
static uint64_t disk_sectors(int fd) {
    off_t end = lseek(fd, 0, SEEK_END);
    return end > 0 ? (uint64_t)end / SECTOR : 0;
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
    /* The running boot disk is refused; a system booted from the ISO has
     * none (no slot), and installs all the same. */
    char slot = running_slot();
    if (slot != 0 && mount_running_esp(slot, running, sizeof running) == 0) {
        release_esp();
        size_t rl = strcspn(running, "/");
        if (strlen(disk) == rl && strncmp(running, disk, rl) == 0) {
            return die("refusing to install over the running boot disk: ", disk);
        }
    }
    if (disk_mounted(disk)) {
        return die("a partition of the disk is mounted (/proc/mounts): ", disk);
    }
    int fd = open(data, O_RDWR);
    if (fd < 0) {
        return die("no such disk: ", data);
    }
    uint64_t total = disk_sectors(fd);
    /* The ESP and some room for the data partition. */
    if (total < 2 * MIB + 512 * MIB + 64 * MIB + 34) {
        close(fd);
        return die("the disk is too small (580 MiB at least): ", disk);
    }
    say("writing the boot disk layout on ", data, NULL);
    uint8_t data_guid[16];
    int rc = write_gpt(fd, total, data_guid);
    close(fd);
    if (rc != 0) {
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
        return die("cannot format the partitions of ", disk);
    }
    mkdirs(NEW_ESP, 1);
    if (mount(esp_dev, NEW_ESP, "fat") != 0) {
        return die("cannot mount the new ESP ", esp_dev);
    }
    char dir[PATH_MAX_GV];
    slot_path(dir, NEW_ESP, 'b', "");
    mkdirs(dir, 1);
    copy_field(esp_part, sizeof esp_part, esp_dev + 5, strlen(esp_dev + 5));
    rc = write_limine(NEW_ESP, &b, url_of);
    if (rc == 0) {
        rc = write_fstab(NEW_ESP, data_guid);
    }
    if (rc == 0) {
        rc = write_slot(NEW_ESP, esp_part, 'a', &b, url_of);
    }
    /* Limine's files too, as the disk has them after the slot's remount. */
    for (int i = 0; rc == 0 && i < b.nesp; i++) {
        char path[PATH_MAX_GV];
        esp_path(path, NEW_ESP, b.esp[i].path, "");
        if (!file_is(path, b.esp[i].size, b.esp[i].csum)) {
            rc = die("the disk does not hold what the list has: ", path);
        }
    }
    umount_dir(NEW_ESP);
#if defined(__x86_64__)
    /* The BIOS stage: Limine's MBR code and its stage 2 in the BIOS boot
     * partition (GPT entry 1), as the host does for the images. */
    if (rc == 0) {
        rc = bios_install(disk);
    }
#endif
    if (rc != 0) {
        return die("the install stopped; the disk is not bootable: ", disk);
    }
    say("installed on ", disk, ": slot a has the release");
    say("  ", b.header, NULL);
    return 0;
}
