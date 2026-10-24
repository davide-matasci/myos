/*
 * get-myos [-r ROOT] [-m MIRROR] [-u] [-l] PACKAGE...
 * get-myos [-r ROOT] -d PACKAGE... | --sync [FILE] | --gc
 * get-myos [-r ROOT] --generations | --rollback [PACKAGE...] | --switch N
 * get-myos [-r ROOT] --global [PACKAGE...] | --unglobal PACKAGE...
 * get-myos [-r ROOT] --root
 * get-myos [-m MIRROR] --upgrade [-f]
 * get-myos [-m MIRROR] --install DISK [--local]
 *
 * Install myos packages: the programs CI builds but the image does not
 * carry (packages/<name>, docs/packages.md), as apps. A package is a gzip
 * tar of the files the port would have in the image (bin/custom/vim,
 * lib/vim/vimrc) and its manifest (`app`), unpacked into a directory of
 * the apps root of its own and of its version, ROOT/<name>-<version>/, and
 * nowhere else: nothing of the system changes. ROOT/<name> is a symlink to
 * the current version; uninstalling is removing it and its versions.
 * `run-myos NAME` runs it, its files seen at their image paths in that
 * process's namespace only (run-myos, docs/packages.md). ROOT is -r, else
 * $MYOS_APPS, else
 * /data/apps when the boot disk's data partition is mounted (it survives
 * a reboot), else /tmp/apps; --root prints it. A package's runtime
 * dependencies (the `deps` field of its index line, from the port's
 * PORT_RDEPS) are installed first.
 *
 * An upgrade unpacks the new version beside the old one and points
 * ROOT/<name> at it only once its checksum matched (a new symlink renamed
 * over the old one): a failed download leaves the old version current.
 * The index names each dependency's version (`x11-libs@<hash>`), and so
 * does the app's manifest, which run-myos binds.
 *
 * ROOT/apps.list lists the apps asked for (`get-myos NAME` adds one, -d
 * takes it out); --sync makes the installed apps what it says, removing
 * those neither listed nor needed. Each command that changes the installed
 * apps writes a generation (ROOT/.get-myos/generations/N: every app, its
 * version, listed or needed); --rollback and --switch N return the whole
 * set to one, apps.list included, and --rollback NAME one app. Versions
 * stay on disk until --gc, which keeps the current generation only.
 *
 * --global NAME binds an app's files (and what it needs) where the image
 * would have them, for every process, until --unglobal NAME (root only:
 * binds need kernel.mounts); /etc/rc binds them again at boot.
 *
 * The mirror holds one index per architecture (<arch>-index.txt: a header
 * naming the build's release and syscall ABI, then name, version, size,
 * SHA-256, file and dependencies of every package), the list of what the
 * image lacks (<arch>-packages.txt) and the tarballs (<arch>-<name>.tar.gz).
 * The index is downloaded once into ROOT/.get-myos/index; -u
 * refreshes it and upgrades the installed packages whose version changed.
 * -l lists the mirror's packages. An index whose ABI is above the running
 * system's (/lib/myos-release, written by the image build) is refused: its
 * programs could call syscalls this kernel lacks.
 *
 * A package streams from curl through gunzip and the tar reader into
 * ROOT/.<name>-<version>.new (nothing is stored: a tmpfs file holds 16 MiB
 * at most, less than some packages); its SHA-256 is checked over the
 * stream, and only a package whose checksum matches is renamed to its
 * version's directory and recorded (ROOT/.get-myos/pkgs/<name> holds the
 * current version). What an install, -u or --sync downloads goes four
 * packages at a time, each in a worker process (fetch_all); the recording
 * stays here, in order, after them. MYOS_MIRROR or -m overrides the default, the
 * project's rolling GitHub release; the full boot test uses the host-served
 * mirror of the build's own packages (http://10.0.2.2:8765).
 *
 * --upgrade and --install put the release's kernel and initramfs
 * (<arch>-boot.txt) into a boot slot of the boot disk (boot.c,
 * docs/install.md); --install --local the running system's, with no
 * mirror (a system booted from the ISO, offline).
 *
 * The download, tar and gzip code is shared with get-alpine (pkgtools.c).
 * Uses fputs, not printf: newlib's printf needs extra soft-float helpers on
 * aarch64 and riscv64.
 */
#include <dirent.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/myos_extra.h> /* lstat, mount */
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#include "boot.h"
#include "pkgtools.h"

#if defined(__x86_64__)
#define MYOS_ARCH "x86_64"
#elif defined(__aarch64__)
#define MYOS_ARCH "aarch64"
#elif defined(__riscv) && __riscv_xlen == 64
#define MYOS_ARCH "riscv64"
#else
#error "no packages for this architecture"
#endif

#define DEFAULT_MIRROR "https://github.com/davide-matasci/myos/releases/download/rolling"

/* The running system's release (src/release.rs), written by the image build. */
#ifndef MYOS_RELEASE_FILE
#define MYOS_RELEASE_FILE "/lib/myos-release"
#endif

static char mirror[200];

/* ROOT/.get-myos/<file> */
static int db_path(char *out, const char *file) {
    char rel[PATH_MAX_GV];
    if (strlen(file) + 32 > sizeof rel) {
        return -1;
    }
    strcpy(rel, ".get-myos/");
    strcat(rel, file);
    return under_root(out, rel);
}

void mirror_url(char *url, size_t cap, const char *file) {
    copy_field(url, cap, mirror, strlen(mirror));
    if (strlen(url) + 1 + strlen(file) < cap) {
        strcat(url, "/");
        strcat(url, file);
    }
}

/* `text` and a newline into `path`, written beside it and renamed over it:
 * a reader sees the old file or the new one. */
static int write_text(const char *path, const char *text) {
    char tmp[PATH_MAX_GV];
    if (strlen(path) + 5 > sizeof tmp) {
        return -1;
    }
    strcpy(tmp, path);
    strcat(tmp, ".new");
    mkdirs(path, 0);
    int fd = open(tmp, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        return -1;
    }
    size_t n = strlen(text);
    int ok = write(fd, text, n) == (ssize_t)n && write(fd, "\n", 1) == 1;
    close(fd);
    if (!ok || rename(tmp, path) != 0) {
        unlink(tmp);
        return -1;
    }
    return 0;
}

/* Fetch FILE of the mirror into ROOT/.get-myos/<db>: beside it first, so
 * that a failed download (offline, at boot) leaves the one there before. */
static int fetch_db(const char *file, const char *db) {
    char url[512], path[PATH_MAX_GV], tmp[PATH_MAX_GV];
    if (db_path(path, db) != 0 || strlen(path) + 5 > sizeof tmp) {
        return die("root path too long", NULL);
    }
    strcpy(tmp, path);
    strcat(tmp, ".new");
    mkdirs(path, 0);
    mirror_url(url, sizeof url, file);
    say("fetching ", url, NULL);
    if (download(url, tmp) != 0) {
        return -1;
    }
    return rename(tmp, path);
}

static int update_index(void) {
    char path[PATH_MAX_GV];
    if (fetch_db(MYOS_ARCH "-index.txt", "index") != 0) {
        return die("cannot download the index", NULL);
    }
    /* The mirror the index came from is the one a later call without -m
     * uses (the --sync at boot). */
    if (db_path(path, "mirror") == 0) {
        write_text(path, mirror);
    }
    /* What there is to install (-l); an older mirror has no such list. */
    if (fetch_db(MYOS_ARCH "-packages.txt", "packages") != 0) {
        if (db_path(path, "packages") == 0) {
            unlink(path);
        }
    }
    return 0;
}

/* The value of `key=` in a "key=value key=value" line, "" when absent. */
static void field_of(const char *line, const char *key, char *out, size_t cap) {
    out[0] = '\0';
    size_t kl = strlen(key);
    for (const char *p = line; *p != '\0'; p++) {
        if ((p == line || p[-1] == ' ') && strncmp(p, key, kl) == 0 && p[kl] == '=') {
            copy_field(out, cap, p + kl + 1, strcspn(p + kl + 1, " \n"));
            return;
        }
    }
}

/* The index header, "# myos release=... commit=... abi=...": release and
 * abi, "" each when the index has no header (an older build's). */
static void index_release(char *release, size_t rcap, char *abi, size_t acap) {
    char index[PATH_MAX_GV], line[256];
    release[0] = abi[0] = '\0';
    FILE *f = db_path(index, "index") == 0 ? fopen(index, "r") : NULL;
    if (f == NULL) {
        return;
    }
    if (fgets(line, sizeof line, f) != NULL && strncmp(line, "# myos ", 7) == 0) {
        field_of(line + 7, "release", release, rcap);
        field_of(line + 7, "abi", abi, acap);
    }
    fclose(f);
}

/* The running system's release file, written by the image build:
 * "release=... commit=... abi=..."; abi is "" when the image has none. */
static void system_release(char *release, size_t rcap, char *abi, size_t acap) {
    char line[256];
    release[0] = abi[0] = '\0';
    FILE *f = fopen(MYOS_RELEASE_FILE, "r");
    if (f == NULL) {
        return;
    }
    if (fgets(line, sizeof line, f) != NULL) {
        field_of(line, "release", release, rcap);
        field_of(line, "abi", abi, acap);
    }
    fclose(f);
}

/* The index's packages were built against a syscall ABI this kernel has:
 * the numbers only grow, so a greater one means calls the kernel lacks. */
static int check_abi(void) {
    char rel[32], abi[16], srel[32], sabi[16];
    index_release(rel, sizeof rel, abi, sizeof abi);
    system_release(srel, sizeof srel, sabi, sizeof sabi);
    if (abi[0] == '\0' || sabi[0] == '\0') {
        return 0; /* an older index or image: nothing to compare */
    }
    if (atoi(abi) > atoi(sabi)) {
        say("the mirror's packages need a newer myos: syscall ABI ", abi, NULL);
        return die("this system has ABI ", sabi);
    }
    return 0;
}

/* One index line: "name version size sha256 file [deps]"; deps are comma
 * separated, "-" (or absent, in an older index) for none. */
typedef struct {
    char name[128], version[64], size[24], csum[72], file[160], deps[256];
} package;

static int find_package(const char *want, package *pkg) {
    char index[PATH_MAX_GV], line[1024];
    if (db_path(index, "index") != 0) {
        return -1;
    }
    FILE *f = fopen(index, "r");
    if (f == NULL) {
        return -1;
    }
    int found = -1;
    while (fgets(line, sizeof line, f) != NULL) {
        size_t n = strcspn(line, " \n");
        if (n != strlen(want) || memcmp(line, want, n) != 0) {
            continue;
        }
        char *fields[6] = {pkg->name, pkg->version, pkg->size, pkg->csum, pkg->file, pkg->deps};
        size_t caps[6] = {sizeof pkg->name, sizeof pkg->version, sizeof pkg->size,
                          sizeof pkg->csum, sizeof pkg->file, sizeof pkg->deps};
        const char *p = line;
        for (int i = 0; i < 6; i++) {
            n = strcspn(p, " \n");
            copy_field(fields[i], caps[i], p, n);
            p += n + (p[n] == ' ');
        }
        if (pkg->deps[0] == '\0') {
            strcpy(pkg->deps, "-");
        }
        found = 0;
        break;
    }
    fclose(f);
    return found;
}

/* ---- unpacking into the app's directory ---------------------------------- */

/* The package being unpacked: its entries go to ROOT/<app>/ (the
 * version's directory being written, ROOT/.<name>-<version>.new). */
static const char *app;

typedef struct {
    int fd;
    int failed;
} unpack;

static int unpack_entry(tar *t, char type, const char *name, const char *link, uint64_t size) {
    unpack *u = t->ctx;
    (void)size;
    while (name[0] == '.' && name[1] == '/') {
        name += 2;
    }
    while (name[0] == '/') {
        name++;
    }
    if (name[0] == '\0' || name[0] == '.' || strstr(name, "/../") != NULL || strncmp(name, "../", 3) == 0) {
        return 0;
    }
    char rel[PATH_MAX_GV], path[PATH_MAX_GV];
    if (strlen(app) + 1 + strlen(name) >= sizeof rel) {
        u->failed = 1;
        return 0;
    }
    strcpy(rel, app);
    strcat(rel, "/");
    strcat(rel, name);
    if (under_root(path, rel) != 0) {
        u->failed = 1;
        return 0;
    }
    size_t pl = strlen(path);
    while (pl > 1 && path[pl - 1] == '/') {
        path[--pl] = '\0';
    }
    if (type == '5') {
        mkdirs(path, 1);
        return 0;
    }
    if (type != '0' && type != '\0' && type != '7' && type != '2') {
        return 0; /* the packer writes regular files and symlinks only */
    }
    mkdirs(path, 0);
    unlink(path);
    if (type == '2') {
        /* An alias of a program: a symlink to the file that has its data,
         * relative to the link's directory. */
        char target[PATH_MAX_GV];
        copy_field(target, sizeof target, link, strlen(link));
        if (symlink(target, path) != 0) {
            say("cannot create symlink ", path, NULL);
            u->failed = 1;
            return 0;
        }
        return 0;
    }
    u->fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0755);
    if (u->fd < 0) {
        say("cannot write ", path, NULL);
        u->failed = 1;
        return 0;
    }
    return 1;
}

static void unpack_data(tar *t, const uint8_t *p, size_t n) {
    unpack *u = t->ctx;
    while (n > 0 && u->fd >= 0) {
        ssize_t w = write(u->fd, p, n);
        if (w <= 0) {
            u->failed = 1;
            break;
        }
        p += w, n -= (size_t)w;
    }
}

static void unpack_end(tar *t) {
    unpack *u = t->ctx;
    if (u->fd >= 0) {
        close(u->fd);
        u->fd = -1;
    }
}

/* One pass over the stream: the compressed bytes into the SHA-256, the
 * inflated ones into the tar reader. */
typedef struct {
    sha256 sha;
    tar t;
    unpack u;
} stream;

static void stream_in(void *ctx, int member, const uint8_t *p, size_t n) {
    (void)member;
    sha256_update(&((stream *)ctx)->sha, p, n);
}

static void stream_out(void *ctx, int member, const uint8_t *p, size_t n) {
    (void)member;
    tar_feed(&((stream *)ctx)->t, p, n);
}

/* Download and unpack `url` into ROOT/<dir>/ in one pass; 0 when curl,
 * the gzip and the tar all ended well, with the stream's SHA-256 in
 * hex[65]. */
static int fetch_unpack(const char *url, const char *dir, char *hex) {
    int pid = 0;
    int fd = download_open(url, &pid);
    if (fd < 0) {
        return -1;
    }
    stream s;
    memset(&s, 0, sizeof s);
    sha256_init(&s.sha);
    s.t.entry = unpack_entry;
    s.t.data = unpack_data;
    s.t.end = unpack_end;
    s.t.ctx = &s.u;
    s.u.fd = -1;
    app = dir;
    int members = gunzip_fd(fd, stream_in, stream_out, &s);
    unpack_end(&s.t);
    free(s.t.meta);
    int rc = download_close(fd, pid);
    sha256_hex(&s.sha, hex);
    if (rc != 0) {
        say("download failed: ", url, NULL);
        return -1;
    }
    if (members < 1 || s.t.failed || s.u.failed) {
        say("bad archive: ", url, NULL);
        return -1;
    }
    return 0;
}

/* The recorded version of an installed package into version[cap]; -1 when
 * the package is not installed. */
static int installed(const char *name, char *version, size_t cap) {
    char rel[PATH_MAX_GV], path[PATH_MAX_GV];
    if (strlen(name) + 8 > sizeof rel) {
        return -1;
    }
    strcpy(rel, "pkgs/");
    strcat(rel, name);
    if (db_path(path, rel) != 0) {
        return -1;
    }
    FILE *f = fopen(path, "r");
    if (f == NULL) {
        return -1;
    }
    version[0] = '\0';
    if (fgets(version, (int)cap, f) != NULL) {
        version[strcspn(version, "\n")] = '\0';
    }
    fclose(f);
    return 0;
}

/* ROOT/.get-myos/pkgs/<name>, the record of an installed package. */
static int record_path(char *out, const char *name) {
    char rel[PATH_MAX_GV];
    if (strlen(name) + 8 > sizeof rel) {
        return -1;
    }
    strcpy(rel, "pkgs/");
    strcat(rel, name);
    return db_path(out, rel);
}

static void mark_installed(const char *name, const char *version) {
    char path[PATH_MAX_GV];
    if (record_path(path, name) == 0) {
        write_text(path, version);
    }
}

/* ---- versions: ROOT/<name>-<version>/, ROOT/<name> the current one ------- */

/* Index versions are input hashes: lowercase hex, which is also what keeps
 * a version from naming anything but its own directory. */
static int is_version(const char *v) {
    if (v[0] == '\0') {
        return 0;
    }
    for (; *v != '\0'; v++) {
        if (!((*v >= '0' && *v <= '9') || (*v >= 'a' && *v <= 'f'))) {
            return 0;
        }
    }
    return 1;
}

/* The version in `entry`, a name in ROOT, when it is one of name's version
 * directories ("<name>-<version>"), else NULL. */
static const char *version_of(const char *entry, const char *name) {
    size_t n = strlen(name);
    if (strncmp(entry, name, n) != 0 || entry[n] != '-' || !is_version(entry + n + 1)) {
        return NULL;
    }
    return entry + n + 1;
}

/* ROOT/<name>-<version><suffix>, with a leading dot when `hidden`. */
static int version_path(char *out, const char *name, const char *version, const char *suffix, int hidden) {
    char rel[PATH_MAX_GV];
    if (strlen(name) + strlen(version) + strlen(suffix) + 3 > sizeof rel) {
        return -1;
    }
    strcpy(rel, hidden ? "." : "");
    strcat(rel, name);
    strcat(rel, "-");
    strcat(rel, version);
    strcat(rel, suffix);
    return under_root(out, rel);
}

/* The version ROOT/<name> points at into version[cap]; -1 when it is not a
 * symlink to one of name's version directories (not installed, or a plain
 * directory as get-myos made them before versions). */
static int current_version(const char *name, char *version, size_t cap) {
    char path[PATH_MAX_GV], target[PATH_MAX_GV];
    if (under_root(path, name) != 0) {
        return -1;
    }
    ssize_t n = readlink(path, target, sizeof target - 1);
    if (n <= 0) {
        return -1;
    }
    target[n] = '\0';
    const char *v = version_of(target, name);
    if (v == NULL) {
        return -1;
    }
    copy_field(version, cap, v, strlen(v));
    return 0;
}

/* rm -r: `path` and what is below it, symlinks removed, not followed (no
 * stat first: unlink removes anything but a directory, which it refuses). */
static int remove_tree(const char *path) {
    if (unlink(path) == 0) {
        return 0;
    }
    DIR *d = opendir(path);
    if (d != NULL) {
        struct dirent *e;
        char sub[PATH_MAX_GV];
        while ((e = readdir(d)) != NULL) {
            if (strcmp(e->d_name, ".") == 0 || strcmp(e->d_name, "..") == 0) {
                continue;
            }
            if (strlen(path) + 1 + strlen(e->d_name) < sizeof sub) {
                strcpy(sub, path);
                strcat(sub, "/");
                strcat(sub, e->d_name);
                remove_tree(sub);
            }
        }
        closedir(d);
    }
    return rmdir(path);
}

/* Point ROOT/<name> at <name>-<version>: a new symlink renamed over the
 * old one, so that ROOT/<name> is always one version or the other. A plain
 * directory there (an app installed before versions) is removed first. */
static int point_at(const char *name, const char *version) {
    char path[PATH_MAX_GV], tmp[PATH_MAX_GV], target[PATH_MAX_GV];
    if (under_root(path, name) != 0 || version_path(tmp, name, version, ".link", 1) != 0 ||
        strlen(name) + 1 + strlen(version) >= sizeof target) {
        return -1;
    }
    strcpy(target, name);
    strcat(target, "-");
    strcat(target, version);
    unlink(tmp);
    if (symlink(target, tmp) != 0) {
        return die("cannot create symlink ", tmp);
    }
    /* A rename does not replace a directory: retry once it is gone. */
    if (rename(tmp, path) != 0 && (remove_tree(path) != 0 || rename(tmp, path) != 0)) {
        unlink(tmp);
        return die("cannot point at the new version: ", path);
    }
    return 0;
}

/* Calls `each` with every version directory of `name` in ROOT. */
static void each_version(const char *name, void (*each)(const char *version, void *ctx), void *ctx) {
    DIR *d = opendir(pkg_root);
    if (d == NULL) {
        return;
    }
    struct dirent *e;
    while ((e = readdir(d)) != NULL) {
        const char *v = version_of(e->d_name, name);
        char path[PATH_MAX_GV];
        struct stat st;
        if (v != NULL && under_root(path, e->d_name) == 0 && lstat(path, &st) == 0 && S_ISDIR(st.st_mode)) {
            each(v, ctx);
        }
    }
    closedir(d);
}

/* Whether ROOT/<name>-<version>/ is there. */
static int has_version(const char *name, const char *version) {
    char dir[PATH_MAX_GV];
    struct stat st;
    return version_path(dir, name, version, "", 0) == 0 && lstat(dir, &st) == 0 && S_ISDIR(st.st_mode);
}

/* ---- sets of apps: what is installed, a generation, apps.list ------------ */

#define MAX_APPS 128

/* An app at a version; `listed`: named in apps.list (asked for, not only
 * needed by another). */
typedef struct {
    char name[64], version[64];
    int listed;
} app_at;

typedef struct {
    int n;
    app_at a[MAX_APPS];
} app_set;

static app_at *set_find(app_set *s, const char *name) {
    for (int i = 0; i < s->n; i++) {
        if (strcmp(s->a[i].name, name) == 0) {
            return &s->a[i];
        }
    }
    return NULL;
}

static app_at *set_add(app_set *s, const char *name, const char *version) {
    app_at *a = set_find(s, name);
    if (a == NULL && s->n < MAX_APPS && strlen(name) < sizeof a->name) {
        a = &s->a[s->n++];
        copy_field(a->name, sizeof a->name, name, strlen(name));
        a->listed = 0;
    }
    if (a != NULL) {
        copy_field(a->version, sizeof a->version, version, strlen(version));
    }
    return a;
}

/* The installed apps, by name: every ROOT/<name> pointing at a version of
 * it. */
static void installed_set(app_set *s) {
    s->n = 0;
    DIR *d = opendir(pkg_root);
    if (d == NULL) {
        return;
    }
    struct dirent *e;
    char version[64];
    while ((e = readdir(d)) != NULL) {
        if (e->d_name[0] != '.' && current_version(e->d_name, version, sizeof version) == 0) {
            set_add(s, e->d_name, version);
        }
    }
    closedir(d);
    for (int i = 1; i < s->n; i++) {
        app_at a = s->a[i];
        int j = i;
        for (; j > 0 && strcmp(s->a[j - 1].name, a.name) > 0; j--) {
            s->a[j] = s->a[j - 1];
        }
        s->a[j] = a;
    }
}

/* The apps `name` needs at `version` (its manifest's `needs` line) into
 * needs[cap]: space separated, each `dep@version`, or `dep` (its current
 * version) in a manifest from before exact versions. */
static void needs_of(const char *name, const char *version, char *needs, size_t cap) {
    char path[PATH_MAX_GV], line[512];
    needs[0] = '\0';
    if (version_path(path, name, version, "/app", 0) != 0) {
        return;
    }
    FILE *f = fopen(path, "r");
    if (f == NULL) {
        return;
    }
    while (fgets(line, sizeof line, f) != NULL) {
        if (strncmp(line, "needs", 5) == 0 && (line[5] == ' ' || line[5] == '\t' || line[5] == '\n')) {
            const char *p = line + 5 + strspn(line + 5, " \t");
            copy_field(needs, cap, p, strcspn(p, "\n"));
            break;
        }
    }
    fclose(f);
}

/* One `dep[@version]` of a needs line into name[64] and version[64] ("" for
 * a bare name); the next one's start, NULL at the end. */
static const char *next_need(const char *p, char *name, char *version) {
    p += strspn(p, " \t,");
    if (*p == '\0') {
        return NULL;
    }
    size_t n = strcspn(p, " \t,");
    size_t at = strcspn(p, "@ \t,");
    copy_field(name, 64, p, at);
    version[0] = '\0';
    if (at < n) {
        copy_field(version, 64, p + at + 1, n - at - 1);
    }
    return p + n;
}

/* Add to `keep` (versions to keep, by `name-version`) the apps that the app
 * at `name`/`version` needs, theirs too; a bare dependency is its current
 * version. */
static void keep_needs(app_set *keep, const char *name, const char *version, int depth) {
    char needs[512], dep[64], v[64], key[160];
    if (depth > 16) {
        return;
    }
    needs_of(name, version, needs, sizeof needs);
    for (const char *p = needs; (p = next_need(p, dep, v)) != NULL;) {
        if (v[0] == '\0' && current_version(dep, v, sizeof v) != 0) {
            continue;
        }
        if (strlen(dep) + strlen(v) + 2 > sizeof key) {
            continue;
        }
        strcpy(key, dep);
        strcat(key, "-");
        strcat(key, v);
        if (set_find(keep, key) == NULL) {
            set_add(keep, key, v);
            keep_needs(keep, dep, v, depth + 1);
        }
    }
}

/* ROOT/apps.list: the apps asked for (`get-myos NAME`, `--sync`), one name
 * per line; `#` starts a comment. */
static int list_path(char *out) {
    return under_root(out, "apps.list");
}

/* The names in the list file `path` into `s`; -1 when it cannot be read. */
static int read_list(const char *path, app_set *s) {
    char line[256];
    s->n = 0;
    FILE *f = fopen(path, "r");
    if (f == NULL) {
        return -1;
    }
    while (fgets(line, sizeof line, f) != NULL) {
        line[strcspn(line, "#\n")] = '\0';
        const char *p = line + strspn(line, " \t");
        size_t n = strcspn(p, " \t");
        char name[64];
        if (n == 0 || n >= sizeof name) {
            continue;
        }
        copy_field(name, sizeof name, p, n);
        app_at *a = set_add(s, name, "");
        if (a != NULL) {
            a->listed = 1;
        }
    }
    fclose(f);
    return 0;
}

/* ROOT/apps.list from the listed apps of `s`. */
static int write_list(const app_set *s) {
    char path[PATH_MAX_GV], text[MAX_APPS * 65 + 1];
    if (list_path(path) != 0) {
        return -1;
    }
    text[0] = '\0';
    for (int i = 0; i < s->n; i++) {
        if (s->a[i].listed) {
            strcat(text, s->a[i].name);
            strcat(text, "\n");
        }
    }
    size_t n = strlen(text);
    if (n > 0) {
        text[n - 1] = '\0'; /* write_text ends it */
    }
    return write_text(path, text);
}

/* Whether app `name` is needed by another installed app (in `s`); its
 * first such dependent into who[64]. */
static int needed(const app_set *s, const char *name, char *who) {
    char needs[512], dep[64], v[64];
    for (int i = 0; i < s->n; i++) {
        if (strcmp(s->a[i].name, name) == 0) {
            continue;
        }
        needs_of(s->a[i].name, s->a[i].version, needs, sizeof needs);
        for (const char *p = needs; (p = next_need(p, dep, v)) != NULL;) {
            if (strcmp(dep, name) == 0) {
                copy_field(who, 64, s->a[i].name, strlen(s->a[i].name));
                return 1;
            }
        }
    }
    return 0;
}

/* The installed apps with their `listed` marks from ROOT/apps.list. With no
 * list yet (apps installed before there was one), the apps no other one
 * needs count as asked for, and the list is written so. */
static void current_set(app_set *s) {
    static app_set list;
    char path[PATH_MAX_GV], who[64];
    installed_set(s);
    if (list_path(path) == 0 && read_list(path, &list) == 0) {
        for (int i = 0; i < s->n; i++) {
            s->a[i].listed = set_find(&list, s->a[i].name) != NULL;
        }
        return;
    }
    for (int i = 0; i < s->n; i++) {
        s->a[i].listed = !needed(s, s->a[i].name, who);
    }
    write_list(s);
}

/* Add `name` to ROOT/apps.list, or take it out (`listed` 0). */
static void list_mark(const char *name, int listed) {
    static app_set s, list;
    char path[PATH_MAX_GV];
    current_set(&s);
    if (list_path(path) != 0 || read_list(path, &list) != 0) {
        return;
    }
    app_at *a = set_find(&list, name);
    if (a == NULL && listed) {
        a = set_add(&list, name, "");
    }
    if (a != NULL && a->listed != listed) {
        a->listed = listed;
        write_list(&list);
    }
}

/* ---- generations: ROOT/.get-myos/generations/N, the current one's N ----- */

/* `n` (> 0) in decimal into num[16]: its start. */
static const char *decimal(int n, char *num) {
    int i = 15;
    num[i] = '\0';
    for (; n > 0 && i > 0; n /= 10) {
        num[--i] = (char)('0' + n % 10);
    }
    return num + i;
}

/* Each command that changes the installed apps writes a generation: every
 * installed app at its version, `listed` or `needed`. `--rollback` and
 * `--switch N` return the whole set to one, apps.list included. */

static int gen_path(char *out, int n) {
    char rel[48];
    char num[16];
    strcpy(rel, "generations/");
    strcat(rel, n > 0 ? decimal(n, num) : "current");
    return db_path(out, rel);
}

static int gen_current(void) {
    char path[PATH_MAX_GV], line[32];
    int n = 0;
    FILE *f = gen_path(path, 0) == 0 ? fopen(path, "r") : NULL;
    if (f != NULL) {
        if (fgets(line, sizeof line, f) != NULL) {
            n = atoi(line);
        }
        fclose(f);
    }
    return n;
}

/* The generations' numbers, ascending, into gens[cap]; their count. */
static int gen_list(int *gens, int cap) {
    char dir[PATH_MAX_GV];
    int n = 0;
    if (gen_path(dir, 0) != 0) {
        return 0;
    }
    *strrchr(dir, '/') = '\0';
    DIR *d = opendir(dir);
    if (d == NULL) {
        return 0;
    }
    struct dirent *e;
    while ((e = readdir(d)) != NULL && n < cap) {
        int g = atoi(e->d_name);
        if (g > 0 && strspn(e->d_name, "0123456789") == strlen(e->d_name)) {
            int i = n++;
            for (; i > 0 && gens[i - 1] > g; i--) {
                gens[i] = gens[i - 1];
            }
            gens[i] = g;
        }
    }
    closedir(d);
    return n;
}

/* Generation `n` into `s`; -1 when there is none. */
static int gen_read(int n, app_set *s) {
    char path[PATH_MAX_GV], line[256], name[64], version[64], how[16];
    s->n = 0;
    FILE *f = gen_path(path, n) == 0 ? fopen(path, "r") : NULL;
    if (f == NULL) {
        return -1;
    }
    while (fgets(line, sizeof line, f) != NULL) {
        const char *p = line;
        size_t a = strcspn(p, " \n");
        copy_field(name, sizeof name, p, a);
        p += a + (p[a] == ' ');
        size_t b = strcspn(p, " \n");
        copy_field(version, sizeof version, p, b);
        p += b + (p[b] == ' ');
        copy_field(how, sizeof how, p, strcspn(p, " \n"));
        app_at *x = name[0] != '\0' && is_version(version) ? set_add(s, name, version) : NULL;
        if (x != NULL) {
            x->listed = strcmp(how, "listed") == 0;
        }
    }
    fclose(f);
    return 0;
}

static int same_set(app_set *a, app_set *b) {
    if (a->n != b->n) {
        return 0;
    }
    for (int i = 0; i < a->n; i++) {
        app_at *x = set_find(b, a->a[i].name);
        if (x == NULL || strcmp(x->version, a->a[i].version) != 0 || x->listed != a->a[i].listed) {
            return 0;
        }
    }
    return 1;
}

static int set_current(int n) {
    char path[PATH_MAX_GV], num[16];
    return gen_path(path, 0) == 0 ? write_text(path, decimal(n, num)) : -1;
}

/* Write the installed set as a new generation and make it the current
 * one, unless it is the current one already. */
static int commit_generation(void) {
    static app_set now, cur;
    static char text[MAX_APPS * 150];
    char path[PATH_MAX_GV];
    int gens[1024];
    current_set(&now);
    int c = gen_current();
    if (c > 0 && gen_read(c, &cur) == 0 && same_set(&now, &cur)) {
        return 0;
    }
    int n = gen_list(gens, 1024);
    int next = (n > 0 ? gens[n - 1] : 0) + 1;
    text[0] = '\0';
    for (int i = 0; i < now.n; i++) {
        strcat(text, now.a[i].name);
        strcat(text, " ");
        strcat(text, now.a[i].version);
        strcat(text, now.a[i].listed ? " listed" : " needed");
        if (i + 1 < now.n) {
            strcat(text, "\n");
        }
    }
    if (gen_path(path, next) != 0 || write_text(path, text) != 0 || set_current(next) != 0) {
        return die("cannot write a generation", NULL);
    }
    return 0;
}

/* ---- global apps: bound where the image would have them --------------- */

/* `get-myos --global NAME` binds an app's files, and those of what it
 * needs, over the system for every process (`mount SRC TARGET bind`), by
 * run-myos's rule: the shallowest path the system lacks, or a file alone
 * in a directory it has. The global apps are ROOT/.get-myos/global/<name>;
 * the binds get-myos made, ROOT/.get-myos/binds (`target source app` a
 * line). After every command that moves versions they are made again for
 * the versions now current (binds name a version's directory: the kernel
 * does not follow ROOT/<name> in a bind's source). Binds need `write` on
 * kernel.mounts: root's. /etc/rc binds them again at boot. */

#define MAX_BINDS 512

typedef struct {
    char target[160], source[PATH_MAX_GV], app[64];
} bind_at;

typedef struct {
    int n;
    bind_at b[MAX_BINDS];
} bind_set;

/* ROOT/.get-myos/global/<name>. */
static int global_path(char *out, const char *name) {
    char rel[PATH_MAX_GV];
    if (strlen(name) + 8 > sizeof rel) {
        return -1;
    }
    strcpy(rel, "global/");
    strcat(rel, name);
    return db_path(out, rel);
}

/* Whether the kernel has `source` bound at `target` (a /proc/mounts line
 * `SOURCE TARGET bind ...`): a record outlives the binds at a reboot. */
static int bound_now(const bind_at *b) {
    char line[600];
    size_t sl = strlen(b->source), tl = strlen(b->target);
    int found = 0;
    FILE *f = fopen("/proc/mounts", "r");
    while (f != NULL && !found && fgets(line, sizeof line, f) != NULL) {
        found = strncmp(line, b->source, sl) == 0 && line[sl] == ' ' && strncmp(line + sl + 1, b->target, tl) == 0 &&
                strncmp(line + sl + 1 + tl, " bind ", 6) == 0;
    }
    if (f != NULL) {
        fclose(f);
    }
    return found;
}

/* The binds get-myos made that are still there. */
static void read_binds(bind_set *s) {
    char path[PATH_MAX_GV], line[600];
    s->n = 0;
    FILE *f = db_path(path, "binds") == 0 ? fopen(path, "r") : NULL;
    if (f == NULL) {
        return;
    }
    while (s->n < MAX_BINDS && fgets(line, sizeof line, f) != NULL) {
        bind_at *b = &s->b[s->n];
        const char *p = line;
        char *fields[3] = {b->target, b->source, b->app};
        size_t caps[3] = {sizeof b->target, sizeof b->source, sizeof b->app};
        for (int i = 0; i < 3; i++) {
            size_t n = strcspn(p, " \n");
            copy_field(fields[i], caps[i], p, n);
            p += n + (p[n] == ' ');
        }
        if (b->target[0] == '/' && b->source[0] == '/' && bound_now(b)) {
            s->n++;
        }
    }
    fclose(f);
}

static int write_binds(const bind_set *s) {
    static char text[MAX_BINDS * 480];
    char path[PATH_MAX_GV];
    if (s->n == 0) {
        return db_path(path, "binds") != 0 || (unlink(path) != 0 && access(path, F_OK) == 0) ? -1 : 0;
    }
    text[0] = '\0';
    for (int i = 0; i < s->n; i++) {
        strcat(text, s->b[i].target);
        strcat(text, " ");
        strcat(text, s->b[i].source);
        strcat(text, " ");
        strcat(text, s->b[i].app);
        if (i + 1 < s->n) {
            strcat(text, "\n");
        }
    }
    return db_path(path, "binds") == 0 ? write_text(path, text) : -1;
}

/* Whether `path` is `under` or below it. */
static int is_under(const char *path, const char *under) {
    size_t n = strlen(under);
    return strncmp(path, under, n) == 0 && (path[n] == '\0' || path[n] == '/');
}

/* Whether the system has `path`, get-myos's own binds (`ours`) aside. */
static int system_has(const char *path, const bind_set *ours) {
    struct stat st;
    for (int i = 0; i < ours->n; i++) {
        if (is_under(path, ours->b[i].target)) {
            return 0;
        }
    }
    return stat(path, &st) == 0;
}

/* The binds of the app in directory `dir` (ROOT/<name>-<version>), below
 * its `rel` (from "/"), added to `want`; -1 when one would be another
 * app's path, said with its owner. */
static int app_binds(const char *app, const char *dir, const char *rel, const bind_set *ours, bind_set *want,
                     int depth) {
    char path[PATH_MAX_GV], target[160];
    if (depth > 32 || strlen(dir) + strlen(rel) + 1 >= sizeof path) {
        return 0;
    }
    strcpy(path, dir);
    strcat(path, rel);
    DIR *d = opendir(path);
    if (d == NULL) {
        return 0;
    }
    int rc = 0;
    struct dirent *e;
    while (rc == 0 && (e = readdir(d)) != NULL) {
        if (e->d_name[0] == '.' && (e->d_name[1] == '\0' || (e->d_name[1] == '.' && e->d_name[2] == '\0'))) {
            continue;
        }
        if (strlen(rel) + 1 + strlen(e->d_name) >= sizeof target) {
            continue;
        }
        strcpy(target, strcmp(rel, "/") == 0 ? "" : rel);
        strcat(target, "/");
        strcat(target, e->d_name);
        /* The manifest and the app's tests are not part of it (run-myos). */
        if (strcmp(target, "/app") == 0 || strcmp(target, "/lib/myos-tests") == 0) {
            continue;
        }
        char source[PATH_MAX_GV];
        struct stat st;
        if (strlen(dir) + strlen(target) >= sizeof source) {
            continue;
        }
        strcpy(source, dir);
        strcat(source, target);
        if (system_has(target, ours) && lstat(source, &st) == 0 && S_ISDIR(st.st_mode)) {
            rc = app_binds(app, dir, target, ours, want, depth + 1);
            continue;
        }
        for (int i = 0; i < want->n; i++) {
            if (is_under(target, want->b[i].target) || is_under(want->b[i].target, target)) {
                say(want->b[i].target, " is provided by ", want->b[i].app);
                say(app, " would provide ", target);
                rc = -1;
                break;
            }
        }
        if (rc == 0 && want->n < MAX_BINDS) {
            bind_at *b = &want->b[want->n++];
            copy_field(b->target, sizeof b->target, target, strlen(target));
            copy_field(b->source, sizeof b->source, source, strlen(source));
            copy_field(b->app, sizeof b->app, app, strlen(app));
        }
    }
    closedir(d);
    return rc;
}

/* `umount TARGET` (the image's; libc has no umount): 0 when it went. */
static int unbind(const char *target) {
    pid_t pid = fork();
    if (pid == 0) {
        execl("/bin/custom/umount", "umount", target, (char *)NULL);
        _exit(127);
    }
    int status = 0;
    return pid > 0 && waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0 ? 0 : -1;
}

/* Bind every global app (and `adding`, NULL for none) and what it needs,
 * at the versions now current (what an app needs: the version its
 * manifest names), and take away the binds no global app has any more.
 * Nothing changes when two apps would have one path, or one app's two
 * versions are needed: -1 then, said. */
static int rebind_global(const char *adding) {
    static app_set apps;
    static bind_set ours, want;
    char path[PATH_MAX_GV], v[64], added[64];
    apps.n = 0;
    if (adding != NULL && current_version(adding, added, sizeof added) != 0) {
        return die("not installed: ", adding);
    }
    if (db_path(path, "global") == 0) {
        DIR *d = opendir(path);
        struct dirent *e;
        while (d != NULL && (e = readdir(d)) != NULL) {
            if (e->d_name[0] == '.' || set_find(&apps, e->d_name) != NULL) {
                continue;
            }
            if (current_version(e->d_name, v, sizeof v) != 0) {
                say(e->d_name, " is not installed: not global any more", NULL);
                if (global_path(path, e->d_name) == 0) {
                    unlink(path);
                }
                continue;
            }
            set_add(&apps, e->d_name, v);
        }
        if (d != NULL) {
            closedir(d);
        }
    }
    /* After the global ones: a path both have is named theirs. */
    if (adding != NULL && set_find(&apps, adding) == NULL) {
        set_add(&apps, adding, added);
    }
    read_binds(&ours);
    if (apps.n == 0 && ours.n == 0) {
        return 0;
    }
    /* The global apps and, transitively, what they need: one version of
     * each (`owner`: who brought it, for the messages). */
    static app_set all;
    all.n = 0;
    for (int i = 0; i < apps.n; i++) {
        set_add(&all, apps.a[i].name, apps.a[i].version);
    }
    for (int i = 0; i < all.n; i++) {
        char needs[512], dep[64], dv[64];
        needs_of(all.a[i].name, all.a[i].version, needs, sizeof needs);
        for (const char *p = needs; (p = next_need(p, dep, dv)) != NULL;) {
            if (dv[0] == '\0' && current_version(dep, dv, sizeof dv) != 0) {
                return die(all.a[i].name, " needs an app that is not installed");
            }
            app_at *have = set_find(&all, dep);
            if (have != NULL && strcmp(have->version, dv) != 0) {
                say(dep, " is needed at two versions: ", have->version);
                return die("and ", dv);
            }
            if (have == NULL && set_add(&all, dep, dv) == NULL) {
                return die("too many global apps", NULL);
            }
        }
    }
    want.n = 0;
    for (int i = 0; i < all.n; i++) {
        char dir[PATH_MAX_GV];
        if (version_path(dir, all.a[i].name, all.a[i].version, "", 0) != 0 ||
            app_binds(all.a[i].name, dir, "/", &ours, &want, 0) != 0) {
            return die("nothing bound", NULL);
        }
    }
    /* Away what goes, then the new ones (a bind replaces one at its
     * target); the record follows what was done. */
    int rc = 0;
    for (int i = 0; i < ours.n; i++) {
        int kept = 0;
        for (int k = 0; k < want.n && !kept; k++) {
            kept = strcmp(want.b[k].target, ours.b[i].target) == 0;
        }
        if (!kept && unbind(ours.b[i].target) != 0) {
            /* Still bound: still recorded, for the next try. */
            say("cannot unbind ", ours.b[i].target, NULL);
            if (want.n < MAX_BINDS) {
                want.b[want.n++] = ours.b[i];
            }
            rc = 1;
        }
    }
    for (int k = 0; k < want.n; k++) {
        int same = 0;
        for (int i = 0; i < ours.n && !same; i++) {
            same = strcmp(want.b[k].target, ours.b[i].target) == 0 && strcmp(want.b[k].source, ours.b[i].source) == 0;
        }
        if (!same && mount(want.b[k].source, want.b[k].target, "bind") != 0) {
            say("cannot bind ", want.b[k].target, " (binds are root's)");
            want.b[k].target[0] = '\0';
            rc = 1;
        }
    }
    int n = 0;
    for (int k = 0; k < want.n; k++) {
        if (want.b[k].target[0] != '\0') {
            want.b[n++] = want.b[k];
        }
    }
    want.n = n;
    if (write_binds(&want) != 0) {
        return die("cannot record the binds", NULL);
    }
    return rc;
}

/* --global NAME: make it global (its record once it is bound). */
static int make_global(const char *name) {
    char path[PATH_MAX_GV];
    if (rebind_global(name) != 0 || global_path(path, name) != 0 || write_text(path, "") != 0) {
        return die("not global: ", name);
    }
    say(name, " is global", NULL);
    return 0;
}

/* --unglobal NAME: its binds and those of what only it needed go. */
static int unmake_global(const char *name) {
    char path[PATH_MAX_GV];
    if (global_path(path, name) != 0 || unlink(path) != 0) {
        return die("not global: ", name);
    }
    if (rebind_global(NULL) != 0) {
        return 1;
    }
    say(name, " is not global any more", NULL);
    return 0;
}

/* Take app `name` out: its pointer (an app installed before versions: its
 * directory) and its record. Its versions stay, for the generations. */
static void unpoint(const char *name) {
    char path[PATH_MAX_GV];
    if (under_root(path, name) == 0) {
        remove_tree(path);
    }
    if (record_path(path, name) == 0) {
        unlink(path);
    }
    /* A global app goes out of the global set with it (its binds go at
     * the end of the command, rebind_global). */
    if (global_path(path, name) == 0) {
        unlink(path);
    }
}

/* --switch N, --rollback: make generation `n` the installed set, apps.list
 * included. Every version it names must be there (`--gc` removes those of
 * other generations), else nothing changes. */
static int switch_to(int n) {
    static app_set want, have;
    char buf[16];
    const char *num = decimal(n, buf);
    if (gen_read(n, &want) != 0) {
        return die("no generation ", num);
    }
    for (int k = 0; k < want.n; k++) {
        if (!has_version(want.a[k].name, want.a[k].version)) {
            say("generation ", num, " needs a version no longer here (--gc):");
            return die(want.a[k].name, "");
        }
    }
    installed_set(&have);
    for (int k = 0; k < want.n; k++) {
        if (point_at(want.a[k].name, want.a[k].version) != 0) {
            return 1;
        }
        mark_installed(want.a[k].name, want.a[k].version);
    }
    for (int k = 0; k < have.n; k++) {
        if (set_find(&want, have.a[k].name) == NULL) {
            unpoint(have.a[k].name);
        }
    }
    if (write_list(&want) != 0 || set_current(n) != 0) {
        return die("cannot record generation ", num);
    }
    say("generation ", num, NULL);
    return 0;
}

/* --rollback without names: the generation before the current one. */
static int rollback_set(void) {
    int gens[1024], c = gen_current(), prev = 0;
    int n = gen_list(gens, 1024);
    for (int k = 0; k < n; k++) {
        if (gens[k] < c) {
            prev = gens[k];
        }
    }
    if (prev == 0) {
        return die("no generation before the current one", NULL);
    }
    return switch_to(prev);
}

/* --generations: one line each, `N` (`*` the current one), then its apps
 * at their versions, those only needed in parentheses. */
static int list_generations(void) {
    static app_set s;
    int gens[1024], c = gen_current();
    int n = gen_list(gens, 1024);
    for (int k = 0; k < n; k++) {
        char num[16];
        if (gen_read(gens[k], &s) != 0) {
            continue;
        }
        fputs(decimal(gens[k], num), stdout);
        fputs(gens[k] == c ? " *" : "  ", stdout);
        for (int a = 0; a < s.n; a++) {
            fputs(s.a[a].listed ? " " : " (", stdout);
            fputs(s.a[a].name, stdout);
            fputs("@", stdout);
            fputs(s.a[a].version, stdout);
            fputs(s.a[a].listed ? "" : ")", stdout);
        }
        fputs("\n", stdout);
    }
    return 0;
}

/* A chain of dependencies longer than this is a loop (the packer refuses
 * one, so an index never has it). */
#define MAX_DEPTH 16

/* Download, check and unpack `pkg` into its version's directory: it streams
 * into ROOT/.<name>-<version>.new, which is renamed to ROOT/<name>-<version>/
 * only once complete and its checksum matched (the current version, if
 * any, stays as it was otherwise). 0, or 1 after saying why. */
static int fetch_version(const package *pkg) {
    char dir[PATH_MAX_GV], tmp[PATH_MAX_GV], url[512], hex[65];
    if (version_path(dir, pkg->name, pkg->version, "", 0) != 0 ||
        version_path(tmp, pkg->name, pkg->version, ".new", 1) != 0) {
        return die("root path too long", NULL);
    }
    mirror_url(url, sizeof url, pkg->file);
    say(pkg->name, " ", pkg->version);
    /* A transient failure should not fail the whole install; a retry
     * starts over in an empty directory. */
    int attempt = 1;
    remove_tree(tmp);
    while (fetch_unpack(url, tmp + strlen(pkg_root) + 1, hex) != 0) {
        remove_tree(tmp);
        if (attempt++ == 3) {
            return die("download failed: ", url);
        }
        say("retrying ", url, NULL);
        sleep(2);
    }
    if (strcmp(hex, pkg->csum) != 0) {
        remove_tree(tmp);
        return die("checksum mismatch: ", pkg->file);
    }
    if (rename(tmp, dir) != 0) {
        remove_tree(tmp);
        return die("cannot rename to ", dir);
    }
    return 0;
}

/* ---- downloads in parallel ----------------------------------------------- */

/* How many packages download and unpack at once (fetch_all). Each one is a
 * worker process of its own, with its own curl, inflating its own stream
 * into its own version's directory: nothing they write is shared, and what
 * the install records (ROOT/<name>, the records, apps.list, the
 * generation) stays with this process, in order, after them. Four: the
 * guest's CPUs on x86_64 and aarch64 (riscv64's two harts share them), and
 * one package's network waits overlap another's inflating. */
#define JOBS 4

/* The packages fetch_all downloaded in this run, and whether each one
 * made it (a worker that failed said why): install() takes them as they
 * are instead of downloading again or saying "back to". */
static package fetch_queue[MAX_APPS];
static int fetch_ok[MAX_APPS];
static int fetch_n;

/* 1: fetched, -1: its fetch failed, 0: fetch_all did not fetch it. */
static int fetch_status(const char *name) {
    for (int i = 0; i < fetch_n; i++) {
        if (strcmp(fetch_queue[i].name, name) == 0) {
            return fetch_ok[i] ? 1 : -1;
        }
    }
    return 0;
}

/* Queue `want` and what it needs, each once, if install() would download
 * it: the index's version is not on disk yet. A name the index lacks or a
 * chain too long is install()'s to report. */
static void queue_fetch(const char *want, int depth) {
    package pkg;
    if (depth > MAX_DEPTH || find_package(want, &pkg) != 0 || !is_version(pkg.version)) {
        return;
    }
    for (int i = 0; i < fetch_n; i++) {
        if (strcmp(fetch_queue[i].name, pkg.name) == 0) {
            return;
        }
    }
    if (strcmp(pkg.deps, "-") != 0) {
        char dep[64], v[64];
        for (const char *p = pkg.deps; (p = next_need(p, dep, v)) != NULL;) {
            queue_fetch(dep, depth + 1);
        }
    }
    if (!has_version(pkg.name, pkg.version) && fetch_n < MAX_APPS) {
        fetch_queue[fetch_n] = pkg;
        fetch_ok[fetch_n] = 0;
        fetch_n++;
    }
}

/* Download what installing `names` (and what they need) will want, JOBS
 * packages at a time, before install() records them one by one. Fewer
 * than two to download: install() does it, as before. */
static void fetch_all(const char *const *names, int n) {
    int first = fetch_n;
    for (int i = 0; i < n; i++) {
        queue_fetch(names[i], 0);
    }
    if (fetch_n - first < 2) {
        fetch_n = first;
        return;
    }
    /* The largest first: the run lasts at least as long as its largest
     * download, which should not start last (an insertion sort: a few
     * dozen packages). */
    for (int i = first + 1; i < fetch_n; i++) {
        package p = fetch_queue[i];
        long size = atol(p.size);
        int j = i;
        while (j > first && atol(fetch_queue[j - 1].size) < size) {
            fetch_queue[j] = fetch_queue[j - 1];
            j--;
        }
        fetch_queue[j] = p;
    }
    int pids[JOBS], at[JOBS], running = 0, next = first;
    /* Nothing buffered may be written twice, by a worker too. */
    fflush(NULL);
    while (next < fetch_n || running > 0) {
        while (running < JOBS && next < fetch_n) {
            int pid = fork();
            if (pid == 0) {
                _exit(fetch_version(&fetch_queue[next]));
            }
            if (pid < 0) {
                /* No worker: install() downloads it itself. */
                fetch_queue[next].name[0] = '\0';
                next++;
                continue;
            }
            pids[running] = pid;
            at[running] = next++;
            running++;
        }
        int status = 0;
        int pid = waitpid(-1, &status, 0);
        if (pid < 0) {
            break;
        }
        for (int j = 0; j < running; j++) {
            if (pids[j] == pid) {
                fetch_ok[at[j]] = WIFEXITED(status) && WEXITSTATUS(status) == 0;
                pids[j] = pids[running - 1];
                at[j] = at[running - 1];
                running--;
                break;
            }
        }
    }
}

/* Bring `want` to the index's version: its dependencies first (depth
 * first, each once, at the versions the index names), then itself when it
 * is not installed or installed at another version: the version is
 * unpacked beside the current one (unless it is still there, from an
 * earlier generation) and ROOT/<name> flipped to it. The versions it
 * replaces stay until `--gc`.
 * `asked` marks a package named on the command line, which says so when
 * there is nothing to do; a dependency already there is silent. */
static int install(const char *want, int depth, int asked) {
    package pkg;
    char have[64];
    if (depth > MAX_DEPTH) {
        return die("dependency chain too long at ", want);
    }
    if (find_package(want, &pkg) != 0) {
        return die("not in the index: ", want);
    }
    if (!is_version(pkg.version)) {
        return die("bad version in the index: ", pkg.name);
    }
    int present = installed(pkg.name, have, sizeof have) == 0;
    if (present && strcmp(have, pkg.version) == 0 && has_version(pkg.name, pkg.version)) {
        if (asked) {
            say(pkg.name, " already installed", NULL);
        }
        return 0;
    }
    if (strcmp(pkg.deps, "-") != 0) {
        char dep[64], v[64];
        for (const char *p = pkg.deps; (p = next_need(p, dep, v)) != NULL;) {
            package d;
            if (install(dep, depth + 1, 0) != 0) {
                return 1;
            }
            /* The index names the version it built against: the one it
             * has, in a whole release; an older one only if still here. */
            if (v[0] != '\0' && !has_version(dep, v) && find_package(dep, &d) == 0) {
                say(pkg.name, " needs ", dep);
                return die("at version ", v);
            }
        }
    }
    int fetched = fetch_status(pkg.name);
    if (fetched < 0) {
        return 1; /* its worker said why */
    }
    if (has_version(pkg.name, pkg.version)) {
        /* Only a complete version is ever renamed to its directory: one
         * fetch_all just made, or one that was here already. */
        if (fetched == 0) {
            say(pkg.name, ": back to ", pkg.version);
        }
    } else if (fetch_version(&pkg) != 0) {
        return 1;
    }
    if (point_at(pkg.name, pkg.version) != 0) {
        return 1;
    }
    mark_installed(pkg.name, pkg.version);
    return 0;
}

/* The one version of `name` on disk besides the current one. */
typedef struct {
    const char *current;
    char other[64];
    int count;
} others;

static void other_one(const char *version, void *ctx) {
    others *o = ctx;
    if (strcmp(version, o->current) != 0) {
        copy_field(o->other, sizeof o->other, version, strlen(version));
        o->count++;
    }
}

/* --rollback NAME: point `name` back at the version it had before: the
 * latest earlier generation's where it differs and is still here, else
 * (an upgrade from before generations) its one other version on disk. */
static int rollback(const char *name) {
    static app_set s;
    char current[64];
    const char *to = NULL;
    int gens[1024], c = gen_current();
    if (current_version(name, current, sizeof current) != 0) {
        return die("not installed: ", name);
    }
    int n = gen_list(gens, 1024);
    for (int k = n - 1; k >= 0 && to == NULL; k--) {
        app_at *a = gens[k] < c && gen_read(gens[k], &s) == 0 ? set_find(&s, name) : NULL;
        if (a != NULL && strcmp(a->version, current) != 0 && has_version(name, a->version)) {
            to = a->version;
        }
    }
    others o = {current, "", 0};
    if (to == NULL) {
        each_version(name, other_one, &o);
        if (o.count == 1) {
            to = o.other;
        }
    }
    if (to == NULL) {
        return die("no earlier version of ", name);
    }
    if (point_at(name, to) != 0) {
        return 1;
    }
    mark_installed(name, to);
    say(name, ": back to ", to);
    return 0;
}

/* -d NAME: take an app out, unless another installed app needs it. Its
 * versions stay until `--gc` (a generation may still bring it back). */
static int uninstall(const char *name) {
    static app_set s;
    char who[64], have[64];
    current_set(&s);
    if (set_find(&s, name) == NULL && installed(name, have, sizeof have) != 0) {
        return die("not installed: ", name);
    }
    if (needed(&s, name, who)) {
        say(name, " is needed by ", who);
        return die("remove that first: get-myos -d ", who);
    }
    list_mark(name, 0);
    unpoint(name);
    say("removed ", name, NULL);
    return 0;
}

/* --gc: drop every generation but the current one, then every version
 * that one does not hold (with what its apps need), and what an
 * interrupted install left (ROOT/.<name>-<version>.new, .link). */
static int collect_garbage(void) {
    static app_set cur, keep;
    char path[PATH_MAX_GV], key[160];
    int gens[1024], c = gen_current();
    if (c == 0 || gen_read(c, &cur) != 0) {
        return die("no current generation", NULL);
    }
    keep.n = 0;
    for (int i = 0; i < cur.n; i++) {
        if (strlen(cur.a[i].name) + strlen(cur.a[i].version) + 2 > sizeof key) {
            continue;
        }
        strcpy(key, cur.a[i].name);
        strcat(key, "-");
        strcat(key, cur.a[i].version);
        set_add(&keep, key, cur.a[i].version);
        keep_needs(&keep, cur.a[i].name, cur.a[i].version, 0);
    }
    int n = gen_list(gens, 1024);
    for (int k = 0; k < n; k++) {
        if (gens[k] != c && gen_path(path, gens[k]) == 0) {
            unlink(path);
        }
    }
    DIR *d = opendir(pkg_root);
    if (d == NULL) {
        return 0;
    }
    struct dirent *e;
    while ((e = readdir(d)) != NULL) {
        const char *name = e->d_name;
        size_t len = strlen(name);
        int stale = name[0] == '.' && len > 5 &&
                    (strcmp(name + len - 4, ".new") == 0 || strcmp(name + len - 5, ".link") == 0) &&
                    strcmp(name, ".get-myos") != 0;
        /* A version directory: `<name>-<hex>`, not an app's pointer. */
        const char *dash = strrchr(name, '-');
        char v[64];
        int version = name[0] != '.' && dash != NULL && dash != name && strlen(dash + 1) >= 8 &&
                      is_version(dash + 1) && current_version(name, v, sizeof v) != 0;
        if ((stale || (version && set_find(&keep, name) == NULL)) && under_root(path, name) == 0) {
            remove_tree(path);
            if (!stale) {
                say("removed ", name, NULL);
            }
        }
    }
    closedir(d);
    return 0;
}

/* -u: every recorded package to the (refreshed) index's version. */
static int upgrade_all(void) {
    char dir[PATH_MAX_GV];
    if (db_path(dir, "pkgs") != 0) {
        return 1;
    }
    DIR *d = opendir(dir);
    if (d == NULL) {
        return 0; /* nothing installed */
    }
    static char names[MAX_APPS][64];
    const char *ptrs[MAX_APPS];
    int n = 0;
    struct dirent *e;
    while (n < MAX_APPS && (e = readdir(d)) != NULL) {
        if (e->d_name[0] != '.' && strstr(e->d_name, ".new") == NULL) {
            copy_field(names[n], sizeof names[n], e->d_name, strlen(e->d_name));
            ptrs[n] = names[n];
            n++;
        }
    }
    closedir(d);
    fetch_all(ptrs, n);
    int rc = 0;
    for (int i = 0; rc == 0 && i < n; i++) {
        rc = install(ptrs[i], 0, 0);
    }
    return rc;
}

/* --sync [FILE]: make the installed apps what the list says (FILE, which
 * becomes ROOT/apps.list, else ROOT/apps.list): install what is missing,
 * take out what is neither listed nor needed by a listed app, upgrade the
 * rest to the index. */
static int sync_list(const char *file) {
    static app_set list, s, keep;
    char path[PATH_MAX_GV];
    if (list_path(path) != 0) {
        return die("root path too long", NULL);
    }
    if (read_list(file != NULL ? file : path, &list) != 0) {
        return die("cannot read ", file != NULL ? file : path);
    }
    if (file != NULL && write_list(&list) != 0) {
        return die("cannot write ", path);
    }
    const char *names[MAX_APPS];
    for (int i = 0; i < list.n; i++) {
        names[i] = list.a[i].name;
    }
    fetch_all(names, list.n);
    for (int i = 0; i < list.n; i++) {
        if (install(list.a[i].name, 0, 0) != 0) {
            return 1;
        }
    }
    installed_set(&s);
    keep.n = 0;
    for (int i = 0; i < list.n; i++) {
        char v[64];
        if (current_version(list.a[i].name, v, sizeof v) == 0) {
            set_add(&keep, list.a[i].name, v);
        }
    }
    /* What the listed apps need, by name, transitively. */
    for (int grew = 1; grew;) {
        grew = 0;
        for (int i = 0; i < keep.n; i++) {
            char needs[512], dep[64], v[64], cv[64];
            needs_of(keep.a[i].name, keep.a[i].version, needs, sizeof needs);
            for (const char *p = needs; (p = next_need(p, dep, v)) != NULL;) {
                if (set_find(&keep, dep) == NULL && current_version(dep, cv, sizeof cv) == 0) {
                    set_add(&keep, dep, cv);
                    grew = 1;
                }
            }
        }
    }
    for (int i = 0; i < s.n; i++) {
        if (set_find(&keep, s.a[i].name) == NULL) {
            unpoint(s.a[i].name);
            say("removed ", s.a[i].name, NULL);
        }
    }
    return upgrade_all();
}

/* The installed versions of a package for -l: the current one first,
 * marked with `*`, then the others, comma separated. */
typedef struct {
    const char *current;
    char out[200];
} listed;

static void list_one(const char *version, void *ctx) {
    listed *l = ctx;
    if (strcmp(version, l->current) != 0 && strlen(l->out) + 1 + strlen(version) < sizeof l->out) {
        strcat(l->out, ",");
        strcat(l->out, version);
    }
}

/* -l: the mirror's packages (packages.txt, else every index entry), one
 * line each: name, version, dependencies, "installed", "upgrade" (installed
 * at another version) or "-", and the installed versions; the releases
 * first. */
static int list_packages(void) {
    char rel[32], abi[16], srel[32], sabi[16], path[PATH_MAX_GV], line[1024];
    index_release(rel, sizeof rel, abi, sizeof abi);
    system_release(srel, sizeof srel, sabi, sizeof sabi);
    fputs("mirror: release ", stdout);
    fputs(rel[0] != '\0' ? rel : "unknown", stdout);
    fputs(" abi ", stdout);
    fputs(abi[0] != '\0' ? abi : "unknown", stdout);
    fputs("\nsystem: release ", stdout);
    fputs(srel[0] != '\0' ? srel : "unknown", stdout);
    fputs(" abi ", stdout);
    fputs(sabi[0] != '\0' ? sabi : "unknown", stdout);
    fputs("\n", stdout);
    FILE *f = db_path(path, "packages") == 0 ? fopen(path, "r") : NULL;
    if (f == NULL) {
        f = db_path(path, "index") == 0 ? fopen(path, "r") : NULL;
    }
    if (f == NULL) {
        return die("no index", NULL);
    }
    while (fgets(line, sizeof line, f) != NULL) {
        package pkg;
        char have[64];
        line[strcspn(line, " \n")] = '\0';
        if (line[0] == '\0' || line[0] == '#' || find_package(line, &pkg) != 0) {
            continue;
        }
        const char *state = "-";
        if (installed(pkg.name, have, sizeof have) == 0) {
            state = strcmp(have, pkg.version) == 0 ? "installed" : "upgrade";
        }
        fputs(pkg.name, stdout);
        fputs(" ", stdout);
        fputs(pkg.version, stdout);
        fputs(" ", stdout);
        fputs(pkg.deps, stdout);
        fputs(" ", stdout);
        fputs(state, stdout);
        char current[64];
        if (current_version(pkg.name, current, sizeof current) == 0) {
            listed l = {current, ""};
            copy_field(l.out, sizeof l.out, current, strlen(current));
            strcat(l.out, "*");
            each_version(pkg.name, list_one, &l);
            fputs(" ", stdout);
            fputs(l.out, stdout);
        }
        fputs("\n", stdout);
    }
    fclose(f);
    return 0;
}

/* --upgrade and --install: the mirror's boot file list (--local: the
 * running system's), then boot.c. */
static int boot(const char *disk, int force, int local) {
    char path[PATH_MAX_GV];
    if (local) {
        if (db_path(path, "local-boot") != 0) {
            return die("root path too long", NULL);
        }
        mkdirs(path, 0);
        if (boot_local_list(path) != 0) {
            return 1;
        }
        return boot_install(disk, path, mirror_url);
    }
    if (fetch_db(MYOS_ARCH "-boot.txt", "boot") != 0 || db_path(path, "boot") != 0) {
        return die("the mirror has no boot files (" MYOS_ARCH "-boot.txt)", NULL);
    }
    return disk != NULL ? boot_install(disk, path, mirror_url) : boot_upgrade(path, mirror_url, force);
}

/* The apps root when neither -r nor $MYOS_APPS names one: /data/apps when
 * the boot disk's data partition is mounted at /data (`mount -a`,
 * docs/install.md), else /tmp/apps (the ISO, on the tmpfs). */
static const char *default_root(void) {
    const char *env = getenv("MYOS_APPS");
    if (env != NULL && env[0] != '\0') {
        return env;
    }
    return data_mounted() ? "/data/apps" : "/tmp/apps";
}

int main(int argc, char **argv) {
    int update = 0, list = 0, upgrade = 0, force = 0, local = 0, root = 0, back = 0, i = 1;
    int remove = 0, gc = 0, sync = 0, gens = 0, to = 0, global = 0, unglobal = 0;
    const char *install_disk = NULL, *m = NULL;
    pkg_prog = "get-myos";
    pkg_root = default_root();
    for (; i < argc && argv[i][0] == '-'; i++) {
        if (strcmp(argv[i], "-r") == 0 && i + 1 < argc) {
            pkg_root = argv[++i];
        } else if (strcmp(argv[i], "-m") == 0 && i + 1 < argc) {
            m = argv[++i];
        } else if (strcmp(argv[i], "-u") == 0) {
            update = 1;
        } else if (strcmp(argv[i], "-l") == 0) {
            list = 1;
        } else if (strcmp(argv[i], "-d") == 0) {
            remove = 1;
        } else if (strcmp(argv[i], "--upgrade") == 0) {
            upgrade = 1;
        } else if (strcmp(argv[i], "-f") == 0) {
            force = 1;
        } else if (strcmp(argv[i], "--install") == 0 && i + 1 < argc) {
            install_disk = argv[++i];
        } else if (strcmp(argv[i], "--local") == 0) {
            local = 1;
        } else if (strcmp(argv[i], "--root") == 0) {
            root = 1;
        } else if (strcmp(argv[i], "--rollback") == 0) {
            back = 1;
        } else if (strcmp(argv[i], "--switch") == 0 && i + 1 < argc) {
            to = atoi(argv[++i]);
        } else if (strcmp(argv[i], "--generations") == 0) {
            gens = 1;
        } else if (strcmp(argv[i], "--gc") == 0) {
            gc = 1;
        } else if (strcmp(argv[i], "--sync") == 0) {
            sync = 1;
        } else if (strcmp(argv[i], "--global") == 0) {
            global = 1;
        } else if (strcmp(argv[i], "--unglobal") == 0) {
            unglobal = 1;
        } else {
            break;
        }
    }
    if (root) {
        fputs(pkg_root, stdout);
        fputs("\n", stdout);
        return 0;
    }
    if ((i >= argc && !update && !list && !upgrade && install_disk == NULL && !back && !gc && !sync && !gens &&
         to <= 0 && !global) ||
        (local && install_disk == NULL) || ((remove || unglobal) && i >= argc) || (sync && argc - i > 1)) {
        fputs("usage: get-myos [-r ROOT] [-m MIRROR] [-u] [-l] PACKAGE...\n"
              "       get-myos [-r ROOT] -d PACKAGE... | --sync [FILE] | --gc\n"
              "       get-myos [-r ROOT] --generations | --rollback [PACKAGE...] | --switch N\n"
              "       get-myos [-r ROOT] --global [PACKAGE...] | --unglobal PACKAGE...\n"
              "       get-myos [-r ROOT] --root\n"
              "       get-myos [-m MIRROR] --upgrade [-f] | --install DISK [--local]\n"
              "  Install myos (" MYOS_ARCH ") packages and what they need as apps, each in\n"
              "  ROOT/NAME-VERSION, ROOT/NAME the current one (ROOT: $MYOS_APPS, else\n"
              "  /data/apps with /data mounted, else /tmp/apps; --root prints it), and add\n"
              "  them to ROOT/apps.list; `run-myos NAME` runs one\n"
              "  -m  the mirror (default $MYOS_MIRROR, else the one the index came from,\n"
              "      else " DEFAULT_MIRROR ")\n"
              "  -u  refresh the index and upgrade the installed packages it changed\n"
              "  -l  list the mirror's packages: version, dependencies, installed or not,\n"
              "      the installed versions (the current one marked *)\n"
              "  -d  remove PACKAGE (refused while another app needs it)\n"
              "  --sync  install, upgrade and remove so that the apps are what\n"
              "          ROOT/apps.list (FILE, which replaces it) asks for\n"
              "  --gc  keep the current generation only, and remove the versions it does\n"
              "        not use\n"
              "  --generations  list the generations of the installed apps (* current)\n"
              "  --rollback  the previous generation; with PACKAGE, that one's version\n"
              "              before its current one\n"
              "  --switch N  make generation N the installed apps\n"
              "  --global  bind PACKAGE's files (and what it needs) where the image would\n"
              "            have them, for every process (root only); no PACKAGE: bind the\n"
              "            global apps again (at boot)\n"
              "  --unglobal  take PACKAGE's binds away\n"
              "  --upgrade  write the mirror's kernel and initramfs into the boot slot that is\n"
              "             not running and boot it by default (-f: even when not newer)\n"
              "  --install  lay the boot disk out on DISK (erasing it) and fill its slot a\n"
              "             (--local: with the running system, no mirror needed)\n",
              stderr);
        return 2;
    }
    if (pkg_root[0] != '/' || strlen(pkg_root) > 100) {
        return die("ROOT must be an absolute path of at most 100 bytes: ", pkg_root);
    }
    /* The mirror: -m, else $MYOS_MIRROR, else the one the index came from,
     * else the project's. */
    char path[PATH_MAX_GV], line[256];
    if (m == NULL) {
        m = getenv("MYOS_MIRROR");
    }
    if ((m == NULL || m[0] == '\0') && db_path(path, "mirror") == 0) {
        FILE *f = fopen(path, "r");
        if (f != NULL) {
            if (fgets(line, sizeof line, f) != NULL && line[0] != '\n') {
                line[strcspn(line, "\n")] = '\0';
                m = line;
            }
            fclose(f);
        }
    }
    copy_field(mirror, sizeof mirror, m != NULL && m[0] != '\0' ? m : DEFAULT_MIRROR, sizeof mirror);
    size_t ml = strlen(mirror);
    while (ml > 1 && mirror[ml - 1] == '/') {
        mirror[--ml] = '\0';
    }
    mkdirs(pkg_root, 1);
    if (upgrade || install_disk != NULL) {
        return boot(install_disk, force, local);
    }
    if (gens) {
        return list_generations();
    }
    /* Each command that moves versions or takes an app out binds the
     * global apps again, at what is now current (rebind_global). */
    if (global || unglobal) {
        if (i >= argc) {
            return rebind_global(NULL) != 0;
        }
        for (; i < argc; i++) {
            if ((global ? make_global(argv[i]) : unmake_global(argv[i])) != 0) {
                return 1;
            }
        }
        return 0;
    }
    if (to > 0) {
        int rc = switch_to(to);
        return rebind_global(NULL) != 0 || rc != 0;
    }
    if (back && i >= argc) {
        int rc = rollback_set();
        return rebind_global(NULL) != 0 || rc != 0;
    }
    if (back || remove) {
        int rc = 0;
        for (; rc == 0 && i < argc; i++) {
            rc = back ? rollback(argv[i]) : uninstall(argv[i]);
        }
        rc |= commit_generation();
        return rebind_global(NULL) != 0 || rc != 0;
    }
    if (gc) {
        return commit_generation() != 0 || collect_garbage() != 0;
    }
    char index[PATH_MAX_GV];
    if (db_path(index, "index") != 0) {
        return die("root path too long", NULL);
    }
    if ((update || sync || access(index, F_OK) != 0) && update_index() != 0) {
        return 1;
    }
    if (list) {
        return list_packages();
    }
    if (check_abi() != 0) {
        return 1;
    }
    int rc = 0;
    if (sync) {
        rc = sync_list(i < argc ? argv[i] : NULL);
    } else {
        if (update) {
            rc = upgrade_all();
        }
        if (rc == 0) {
            fetch_all((const char *const *)(argv + i), argc - i);
        }
        for (; rc == 0 && i < argc; i++) {
            rc = install(argv[i], 0, 1);
            if (rc == 0) {
                list_mark(argv[i], 1);
            }
        }
    }
    /* What changed is a generation, even when a later step failed. */
    rc |= commit_generation();
    if (rebind_global(NULL) != 0 || rc != 0) {
        return 1;
    }
    say("done", NULL, NULL);
    return 0;
}
