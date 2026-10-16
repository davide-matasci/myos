/*
 * get-myos [-r ROOT] [-m MIRROR] [-u] [-l] PACKAGE...
 * get-myos [-r ROOT] --rollback PACKAGE...
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
 * The version it replaced is kept, older ones removed; --rollback points
 * back at it.
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
 * current version). MYOS_MIRROR or -m overrides the default, the
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
#include <sys/myos_extra.h> /* lstat */
#include <sys/stat.h>
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

/* Fetch FILE of the mirror into ROOT/.get-myos/<db>. */
static int fetch_db(const char *file, const char *db) {
    char url[512], path[PATH_MAX_GV];
    if (db_path(path, db) != 0) {
        return die("root path too long", NULL);
    }
    mkdirs(path, 0);
    mirror_url(url, sizeof url, file);
    say("fetching ", url, NULL);
    return download(url, path);
}

static int update_index(void) {
    if (fetch_db(MYOS_ARCH "-index.txt", "index") != 0) {
        return die("cannot download the index", NULL);
    }
    /* What there is to install (-l); an older mirror has no such list. */
    if (fetch_db(MYOS_ARCH "-packages.txt", "packages") != 0) {
        char path[PATH_MAX_GV];
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

static void mark_installed(const char *name, const char *version) {
    char rel[PATH_MAX_GV], path[PATH_MAX_GV];
    strcpy(rel, "pkgs/");
    strcat(rel, name);
    if (db_path(path, rel) != 0) {
        return;
    }
    mkdirs(path, 0);
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd >= 0) {
        write(fd, version, strlen(version));
        write(fd, "\n", 1);
        close(fd);
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

/* The versions prune() keeps, and the package's name. */
typedef struct {
    const char *name, *keep, *prev;
} kept;

static void prune_one(const char *version, void *ctx) {
    kept *k = ctx;
    char path[PATH_MAX_GV];
    if (strcmp(version, k->keep) != 0 && strcmp(version, k->prev) != 0 &&
        version_path(path, k->name, version, "", 0) == 0) {
        remove_tree(path);
    }
}

/* Remove name's version directories but `keep` (the current one) and
 * `prev` (the one it replaced, for --rollback). */
static void prune(const char *name, const char *keep, const char *prev) {
    kept k = {name, keep, prev};
    each_version(name, prune_one, &k);
}

/* A chain of dependencies longer than this is a loop (the packer refuses
 * one, so an index never has it). */
#define MAX_DEPTH 16

/* Bring `want` to the index's version: its dependencies first (depth
 * first, each once), then itself when it is not installed or installed at
 * another version: the version is unpacked beside the current one (unless
 * it is still there, the one a rollback left) and ROOT/<name> flipped to it.
 * `asked` marks a package named on the command line, which says so when
 * there is nothing to do; a dependency already there is silent. */
static int install(const char *want, int depth, int asked) {
    package pkg;
    char have[64], prev[64], dir[PATH_MAX_GV], tmp[PATH_MAX_GV];
    struct stat st;
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
    if (present && strcmp(have, pkg.version) == 0) {
        if (asked) {
            say(pkg.name, " already installed", NULL);
        }
        return 0;
    }
    if (strcmp(pkg.deps, "-") != 0) {
        /* Not strtok: the recursion would share its state. */
        for (char *d = pkg.deps; d != NULL && *d != '\0';) {
            char *comma = strchr(d, ',');
            if (comma != NULL) {
                *comma = '\0';
            }
            if (install(d, depth + 1, 0) != 0) {
                return 1;
            }
            d = comma != NULL ? comma + 1 : NULL;
        }
    }
    if (version_path(dir, pkg.name, pkg.version, "", 0) != 0 ||
        version_path(tmp, pkg.name, pkg.version, ".new", 1) != 0) {
        return die("root path too long", NULL);
    }
    if (current_version(pkg.name, prev, sizeof prev) != 0) {
        prev[0] = '\0';
    }
    if (lstat(dir, &st) == 0 && S_ISDIR(st.st_mode)) {
        /* Only a complete version is ever renamed to its directory. */
        say(pkg.name, ": back to ", pkg.version);
    } else {
        char url[512], hex[65];
        mirror_url(url, sizeof url, pkg.file);
        say(pkg.name, " ", pkg.version);
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
        if (strcmp(hex, pkg.csum) != 0) {
            /* The current version, if any, stays as it was. */
            remove_tree(tmp);
            return die("checksum mismatch: ", pkg.file);
        }
        if (rename(tmp, dir) != 0) {
            remove_tree(tmp);
            return die("cannot rename to ", dir);
        }
    }
    if (point_at(pkg.name, pkg.version) != 0) {
        return 1;
    }
    mark_installed(pkg.name, pkg.version);
    /* Older versions go once the pointer moved; without a move (the
     * record alone was off) nothing is known of the previous one. */
    if (strcmp(prev, pkg.version) != 0) {
        prune(pkg.name, pkg.version, prev);
    }
    return 0;
}

/* The other version of `name` than the current one: what --rollback
 * points at. */
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

/* --rollback: point `name` at its other installed version (the one an
 * upgrade replaced; a second rollback goes forward again). */
static int rollback(const char *name) {
    char current[64];
    if (current_version(name, current, sizeof current) != 0) {
        return die("no versions of ", name);
    }
    others o = {current, "", 0};
    each_version(name, other_one, &o);
    if (o.count == 0) {
        return die("no other version of ", name);
    }
    if (o.count > 1) {
        return die("several other versions of ", name);
    }
    if (point_at(name, o.other) != 0) {
        return 1;
    }
    mark_installed(name, o.other);
    say(name, ": back to ", o.other);
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
    int rc = 0;
    struct dirent *e;
    while (rc == 0 && (e = readdir(d)) != NULL) {
        if (e->d_name[0] != '.') {
            rc = install(e->d_name, 0, 0);
        }
    }
    closedir(d);
    return rc;
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
    const char *install_disk = NULL;
    const char *m = getenv("MYOS_MIRROR");
    pkg_prog = "get-myos";
    pkg_root = default_root();
    copy_field(mirror, sizeof mirror, m != NULL ? m : DEFAULT_MIRROR, sizeof mirror);
    for (; i < argc && argv[i][0] == '-'; i++) {
        if (strcmp(argv[i], "-r") == 0 && i + 1 < argc) {
            pkg_root = argv[++i];
        } else if (strcmp(argv[i], "-m") == 0 && i + 1 < argc) {
            copy_field(mirror, sizeof mirror, argv[++i], sizeof mirror);
        } else if (strcmp(argv[i], "-u") == 0) {
            update = 1;
        } else if (strcmp(argv[i], "-l") == 0) {
            list = 1;
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
        } else {
            break;
        }
    }
    if (root) {
        fputs(pkg_root, stdout);
        fputs("\n", stdout);
        return 0;
    }
    if ((i >= argc && !update && !list && !upgrade && install_disk == NULL) || (local && install_disk == NULL) ||
        (back && i >= argc)) {
        fputs("usage: get-myos [-r ROOT] [-m MIRROR] [-u] [-l] PACKAGE...\n"
              "       get-myos [-r ROOT] --rollback PACKAGE...\n"
              "       get-myos [-r ROOT] --root\n"
              "       get-myos [-m MIRROR] --upgrade [-f] | --install DISK [--local]\n"
              "  Install myos (" MYOS_ARCH ") packages and what they need as apps, each in\n"
              "  ROOT/NAME-VERSION, ROOT/NAME the current one (ROOT: $MYOS_APPS, else\n"
              "  /data/apps with /data mounted, else /tmp/apps; --root prints it);\n"
              "  `run-myos NAME` runs one\n"
              "  -m  the mirror (default $MYOS_MIRROR, else " DEFAULT_MIRROR ")\n"
              "  -u  refresh the index and upgrade the installed packages it changed\n"
              "  -l  list the mirror's packages: version, dependencies, installed or not,\n"
              "      the installed versions (the current one marked *)\n"
              "  --rollback  point each PACKAGE back at the version an upgrade replaced\n"
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
    size_t ml = strlen(mirror);
    while (ml > 1 && mirror[ml - 1] == '/') {
        mirror[--ml] = '\0';
    }
    mkdirs(pkg_root, 1);
    if (back) {
        for (; i < argc; i++) {
            if (rollback(argv[i]) != 0) {
                return 1;
            }
        }
        return 0;
    }
    if (upgrade || install_disk != NULL) {
        return boot(install_disk, force, local);
    }
    char index[PATH_MAX_GV];
    if (db_path(index, "index") != 0) {
        return die("root path too long", NULL);
    }
    if ((update || access(index, F_OK) != 0) && update_index() != 0) {
        return 1;
    }
    if (list) {
        return list_packages();
    }
    if (check_abi() != 0) {
        return 1;
    }
    if (update && upgrade_all() != 0) {
        return 1;
    }
    for (; i < argc; i++) {
        if (install(argv[i], 0, 1) != 0) {
            return 1;
        }
    }
    say("done", NULL, NULL);
    return 0;
}
