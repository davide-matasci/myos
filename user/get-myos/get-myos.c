/*
 * get-myos [-r ROOT] [-m MIRROR] [-u] [-l] PACKAGE...
 *
 * Install myos packages: the programs CI builds but the image does not
 * carry (packages/<name>, docs/packages.md). A package is a gzip tar of the
 * files the port would have in the image (bin/custom/vim, lib/vim/vimrc),
 * unpacked under ROOT (default /tmp/pkg, on the tmpfs) and bind-mounted
 * where the image would have them (a directory the image lacks, or a file
 * into one it has), so programs find their files at the usual paths and
 * PATH needs no change. A package's runtime dependencies (the `deps` field
 * of its index line, from the port's PORT_RDEPS) are installed first.
 *
 * The mirror holds one index per architecture (<arch>-index.txt: a header
 * naming the build's release and syscall ABI, then name, version, size,
 * SHA-256, file and dependencies of every package), the list of what the
 * image lacks (<arch>-packages.txt) and the tarballs (<arch>-<name>.tar.gz).
 * The index is downloaded once into ROOT/var/lib/get-myos/index; -u
 * refreshes it and upgrades the installed packages whose version changed.
 * -l lists the mirror's packages. An index whose ABI is above the running
 * system's (/lib/myos-release, written by the image build) is refused: its
 * programs could call syscalls this kernel lacks.
 *
 * A package streams from curl through gunzip and the tar reader into ROOT
 * (nothing is stored: a tmpfs file holds 16 MiB at most, less than some
 * packages); its SHA-256 is checked over the stream, and only a package
 * whose checksum matches is bound and recorded (ROOT/var/lib/get-myos/pkgs/
 * <name> holds its version). MYOS_MIRROR or -m overrides the default, the
 * project's rolling GitHub release; the full boot test uses the host-served
 * mirror of the build's own packages (http://10.0.2.2:8765).
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
#include <sys/stat.h>
#include <unistd.h>

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

/* libgloss/myos mount(2): SYS_MOUNT; "bind" makes SOURCE visible at TARGET. */
int mount(const char *source, const char *target, const char *fstype, ...);

static char mirror[200];

/* ROOT/var/lib/get-myos/<file> */
static int db_path(char *out, const char *file) {
    char rel[PATH_MAX_GV];
    if (strlen(file) + 32 > sizeof rel) {
        return -1;
    }
    strcpy(rel, "var/lib/get-myos/");
    strcat(rel, file);
    return under_root(out, rel);
}

static void mirror_url(char *url, size_t cap, const char *file) {
    copy_field(url, cap, mirror, strlen(mirror));
    if (strlen(url) + 1 + strlen(file) < cap) {
        strcat(url, "/");
        strcat(url, file);
    }
}

/* Fetch FILE of the mirror into ROOT/var/lib/get-myos/<db>. */
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

/* ---- unpacking into the root, then binding -------------------------------- */

/* What a package's entries are bound as: the first directory of the entry's
 * path that the running system does not have (lib/vim for lib/vim/vimrc,
 * lib/os-test for everything under it), or the file itself when all its
 * directories exist (bin/custom/vim: /bin/custom is a read-only tree of the
 * image with other programs in it; lib/myos-tests/ports/2-vim.sh lands next
 * to the image's test scripts). */
#define MAX_BINDS 256
static char binds[MAX_BINDS][PATH_MAX_GV];
static size_t nbinds;

static void note_bind(const char *name) {
    char rel[PATH_MAX_GV], abs[PATH_MAX_GV + 1];
    struct stat st;
    copy_field(rel, sizeof rel, name, strlen(name));
    for (char *slash = strchr(rel, '/'); slash != NULL; slash = strchr(slash + 1, '/')) {
        *slash = '\0';
        abs[0] = '/';
        strcpy(abs + 1, rel);
        int have = stat(abs, &st) == 0;
        *slash = '/';
        if (!have) {
            *slash = '\0';
            break;
        }
    }
    for (size_t i = 0; i < nbinds; i++) {
        if (strcmp(binds[i], rel) == 0) {
            return;
        }
    }
    if (nbinds < MAX_BINDS) {
        strcpy(binds[nbinds++], rel);
    }
}

typedef struct {
    int fd;
    int failed;
} unpack;

static int unpack_entry(tar *t, char type, const char *name, const char *link, uint64_t size) {
    unpack *u = t->ctx;
    (void)link;
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
    char path[PATH_MAX_GV];
    if (under_root(path, name) != 0) {
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
    if (type != '0' && type != '\0' && type != '7') {
        return 0; /* the packer writes regular files only */
    }
    mkdirs(path, 0);
    unlink(path);
    u->fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0755);
    if (u->fd < 0) {
        say("cannot write ", path, NULL);
        u->failed = 1;
        return 0;
    }
    note_bind(name);
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

/* Download and unpack `url` into the root in one pass; 0 when curl, the
 * gzip and the tar all ended well, with the stream's SHA-256 in hex[65]. */
static int fetch_unpack(const char *url, char *hex) {
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
    nbinds = 0;
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

static int bind_all(void) {
    for (size_t i = 0; i < nbinds; i++) {
        char src[PATH_MAX_GV], tgt[PATH_MAX_GV];
        if (under_root(src, binds[i]) != 0 || strlen(binds[i]) + 2 > sizeof tgt) {
            return die("path too long: ", binds[i]);
        }
        tgt[0] = '/';
        strcpy(tgt + 1, binds[i]);
        if (mount(src, tgt, "bind") != 0) {
            say("cannot bind ", src, tgt);
            return 1;
        }
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

/* A chain of dependencies longer than this is a loop (the packer refuses
 * one, so an index never has it). */
#define MAX_DEPTH 16

/* Bring `want` to the index's version: its dependencies first (depth
 * first, each once), then itself when it is not installed or installed at
 * another version (an upgrade: the new files replace the old under ROOT,
 * the binds are by path and keep pointing at them, and are redone anyway).
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
    char url[512], hex[65];
    mirror_url(url, sizeof url, pkg.file);
    say(pkg.name, " ", present ? "upgrade" : pkg.version);
    /* A transient failure should not fail the whole install; a retry
     * rewrites the files of the attempt before it. */
    int attempt = 1;
    while (fetch_unpack(url, hex) != 0) {
        if (attempt++ == 3) {
            return die("download failed: ", url);
        }
        say("retrying ", url, NULL);
        sleep(2);
    }
    if (strcmp(hex, pkg.csum) != 0) {
        /* The files are under ROOT but not bound nor recorded. */
        return die("checksum mismatch: ", pkg.file);
    }
    if (bind_all() != 0) {
        return 1;
    }
    mark_installed(pkg.name, pkg.version);
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

/* -l: the mirror's packages (packages.txt, else every index entry), one
 * line each: name, version, dependencies, "installed", "upgrade" (installed
 * at another version) or "-"; the releases first. */
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
        fputs("\n", stdout);
    }
    fclose(f);
    return 0;
}

int main(int argc, char **argv) {
    int update = 0, list = 0, i = 1;
    const char *m = getenv("MYOS_MIRROR");
    pkg_prog = "get-myos";
    pkg_root = "/tmp/pkg";
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
        } else {
            break;
        }
    }
    if (i >= argc && !update && !list) {
        fputs("usage: get-myos [-r ROOT] [-m MIRROR] [-u] [-l] PACKAGE...\n"
              "  Install myos (" MYOS_ARCH ") packages and what they need into ROOT (default\n"
              "  /tmp/pkg) and bind their files where the image has them (/bin/custom/NAME, /lib/NAME)\n"
              "  -m  the mirror (default $MYOS_MIRROR, else " DEFAULT_MIRROR ")\n"
              "  -u  refresh the index and upgrade the installed packages it changed\n"
              "  -l  list the mirror's packages: version, dependencies, installed or not\n",
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
