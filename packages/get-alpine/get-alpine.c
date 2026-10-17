/*
 * get-alpine [-r ROOT] [-u] [-n] [PACKAGE...]
 *
 * Install Alpine Linux packages, with their run-time dependencies, into a
 * Linux root (default /data/alpine with the boot disk's data partition
 * mounted at /data, else /tmp/alpine), to run them with
 *     linux --root ROOT PROGRAM [ARG...]
 * Part of the optional Linux compatibility layer (docs/linux-compat.md):
 * nothing from Alpine is in the image, everything is fetched at run time.
 *
 * Alpine's own package manager does the installing: a root is bootstrapped
 * with apk-tools and what it needs to run (the `base` set below), fetched,
 * checked and unpacked here, then
 *     linux --root ROOT apk add --initdb BASE... PACKAGE...
 * installs the packages asked for under the Linux layer: apk resolves the
 * dependencies, checks the repository signatures (alpine-keys) and the
 * packages, runs their install scripts and triggers, and records them in
 * its database (ROOT/lib/apk/db, ROOT/etc/apk/world), so `apk upgrade`,
 * `apk del` and the rest work on the root too. -u refreshes the indexes.
 * -n leaves apk out: the packages are fetched, checked and unpacked here,
 * without their scripts (what alpine-disk.sh does on the host, which has
 * no layer to run apk under).
 *
 * The native part: the repository indexes (main and community:
 * APKINDEX.tar.gz) are downloaded once and reduced to
 * ROOT/.get-alpine/index, one line per package: repository, name,
 * version, checksum, dependencies, provides. Dependencies
 * ("so:libonig.so.5", "cmd:sh", names) are resolved through package names
 * and provides. A package (.apk) is three concatenated gzip streams:
 * signature, control (.PKGINFO) and data. The index's checksum is the
 * SHA-1 of the control stream, whose .PKGINFO holds the SHA-256 of the
 * data stream ("datahash"): both are checked before anything is unpacked
 * into ROOT. (The repository signature is not checked here; the index
 * comes over HTTPS. apk checks it afterwards, with the keys the bootstrap
 * unpacked.) What the bootstrap unpacked is recorded in
 * ROOT/.get-alpine/pkgs, so a second run does not fetch it again; apk's
 * first run installs the same packages over it, into its database.
 *
 * Alpine has x86_64, aarch64 and riscv64 repositories. ALPINE_MIRROR
 * overrides https://dl-cdn.alpinelinux.org/alpine and ALPINE_BRANCH
 * latest-stable.
 *
 * The download, tar and gzip code is shared with get-myos
 * (user/get-myos/pkgtools.c). Uses fputs, not printf: newlib's printf
 * needs extra soft-float helpers on aarch64 and riscv64.
 */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#include "pkgtools.h"

/* The repository of the machine it runs on; alpine-disk.sh builds it for
 * the host with another one (-DALPINE_ARCH). */
#if defined(ALPINE_ARCH)
#elif defined(__x86_64__)
#define ALPINE_ARCH "x86_64"
#elif defined(__aarch64__)
#define ALPINE_ARCH "aarch64"
#elif defined(__riscv) && __riscv_xlen == 64
#define ALPINE_ARCH "riscv64"
#else
#error "no Alpine repository for this architecture"
#endif

#define DEFAULT_MIRROR "https://dl-cdn.alpinelinux.org/alpine"
#define DEFAULT_BRANCH "latest-stable"
static char mirror[160], branch[64];
static const char *const repos[] = {"main", "community"};
/* What apk needs to run in the root: apk itself, the keys it checks the
 * repositories with, /bin/sh and the tools install scripts use, the users
 * and directories they expect, the CAs of every TLS client (what OpenSSL's
 * /etc/ssl/cert.pem points at; python3, curl or wget alone do not pull
 * them in). Their dependencies (musl, libapk, libcrypto3, ...) follow. */
static const char *const base[] = {"apk-tools", "alpine-keys", "busybox", "busybox-binsh", "alpine-baselayout",
                                   "ca-certificates-bundle"};
#define NBASE (sizeof base / sizeof base[0])

/* ---- sha1 --------------------------------------------------------------- */

typedef struct {
    uint32_t h[5];
    uint64_t len;
    uint8_t buf[64];
    size_t n;
} sha1;

#define ROL(x, n) (((x) << (n)) | ((x) >> (32 - (n))))

static void sha1_block(sha1 *s, const uint8_t *p) {
    uint32_t w[80], a = s->h[0], b = s->h[1], c = s->h[2], d = s->h[3], e = s->h[4];
    for (int i = 0; i < 16; i++) {
        w[i] = (uint32_t)p[4 * i] << 24 | (uint32_t)p[4 * i + 1] << 16 | (uint32_t)p[4 * i + 2] << 8 |
               p[4 * i + 3];
    }
    for (int i = 16; i < 80; i++) {
        w[i] = ROL(w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16], 1);
    }
    for (int i = 0; i < 80; i++) {
        uint32_t f, k;
        if (i < 20) {
            f = (b & c) | (~b & d), k = 0x5a827999;
        } else if (i < 40) {
            f = b ^ c ^ d, k = 0x6ed9eba1;
        } else if (i < 60) {
            f = (b & c) | (b & d) | (c & d), k = 0x8f1bbcdc;
        } else {
            f = b ^ c ^ d, k = 0xca62c1d6;
        }
        uint32_t t = ROL(a, 5) + f + e + k + w[i];
        e = d, d = c, c = ROL(b, 30), b = a, a = t;
    }
    s->h[0] += a, s->h[1] += b, s->h[2] += c, s->h[3] += d, s->h[4] += e;
}

static void sha1_init(sha1 *s) {
    static const uint32_t iv[5] = {0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0};
    memcpy(s->h, iv, sizeof iv);
    s->len = 0;
    s->n = 0;
}

static void sha1_update(sha1 *s, const uint8_t *p, size_t n) {
    s->len += n;
    while (n > 0) {
        size_t k = 64 - s->n < n ? 64 - s->n : n;
        memcpy(s->buf + s->n, p, k);
        s->n += k, p += k, n -= k;
        if (s->n == 64) {
            sha1_block(s, s->buf);
            s->n = 0;
        }
    }
}

static void sha1_final(sha1 *s, uint8_t out[20]) {
    uint64_t bits = s->len * 8;
    uint8_t pad = 0x80, zero = 0, lenb[8];
    sha1_update(s, &pad, 1);
    while (s->n != 56) {
        sha1_update(s, &zero, 1);
    }
    for (int i = 0; i < 8; i++) {
        lenb[i] = (uint8_t)(bits >> (56 - 8 * i));
    }
    sha1_update(s, lenb, 8);
    for (int i = 0; i < 20; i++) {
        out[i] = (uint8_t)(s->h[i / 4] >> (24 - 8 * (i % 4)));
    }
}

/* Base64 of 20 bytes (as in APKINDEX "C:Q1<base64>"): 28 chars + NUL. */
static void base64_20(const uint8_t in[20], char out[29]) {
    static const char tbl[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    int o = 0;
    for (int i = 0; i < 21; i += 3) {
        uint32_t v = (uint32_t)in[i] << 16 | (i + 1 < 20 ? (uint32_t)in[i + 1] << 8 : 0) |
                     (i + 2 < 20 ? in[i + 2] : 0);
        out[o++] = tbl[v >> 18 & 63];
        out[o++] = tbl[v >> 12 & 63];
        out[o++] = i + 1 < 20 ? tbl[v >> 6 & 63] : '=';
        out[o++] = i + 2 < 20 ? tbl[v & 63] : '=';
    }
    out[28] = '\0';
}

/* ---- the repository indexes --------------------------------------------- */

/* APKINDEX: records of "X:value" lines separated by blank lines. Each
 * record becomes "repo name version checksum|deps|provides\n". */
typedef struct {
    FILE *out;
    const char *repo;
    char line[16384];
    size_t ln;
    int overflow;
    char name[128], version[96], csum[64];
    char *deps, *provides;
    long packages;
} apkindex;

static void field(char **dst, const char *v) {
    free(*dst);
    *dst = strdup(v);
}

static void index_line(apkindex *x) {
    x->line[x->ln] = '\0';
    if (x->ln == 0) {
        if (x->name[0] != '\0' && x->version[0] != '\0' && x->csum[0] != '\0') {
            fputs(x->repo, x->out);
            fputs(" ", x->out);
            fputs(x->name, x->out);
            fputs(" ", x->out);
            fputs(x->version, x->out);
            fputs(" ", x->out);
            fputs(x->csum, x->out);
            fputs("|", x->out);
            fputs(x->deps != NULL ? x->deps : "", x->out);
            fputs("|", x->out);
            fputs(x->provides != NULL ? x->provides : "", x->out);
            fputs("\n", x->out);
            x->packages++;
        }
        x->name[0] = x->version[0] = x->csum[0] = '\0';
        field(&x->deps, "");
        field(&x->provides, "");
        return;
    }
    if (x->overflow || x->ln < 2 || x->line[1] != ':') {
        return;
    }
    const char *v = x->line + 2;
    switch (x->line[0]) {
    case 'P':
        copy_field(x->name, sizeof x->name, v, strlen(v));
        break;
    case 'V':
        copy_field(x->version, sizeof x->version, v, strlen(v));
        break;
    case 'C':
        copy_field(x->csum, sizeof x->csum, v, strlen(v));
        break;
    case 'D':
        field(&x->deps, v);
        break;
    case 'p':
        field(&x->provides, v);
        break;
    }
}

static void index_text(apkindex *x, const uint8_t *p, size_t n) {
    for (size_t i = 0; i < n; i++) {
        if (p[i] == '\n') {
            index_line(x);
            x->ln = 0;
            x->overflow = 0;
        } else if (x->ln + 1 < sizeof x->line) {
            x->line[x->ln++] = (char)p[i];
        } else {
            x->overflow = 1; /* a field we do not keep can be longer */
        }
    }
}

static int index_entry(tar *t, char type, const char *name, const char *link, uint64_t size) {
    (void)link;
    (void)size;
    (void)t;
    return (type == '0' || type == '\0') && strcmp(name, "APKINDEX") == 0;
}

static void index_data(tar *t, const uint8_t *p, size_t n) {
    index_text((apkindex *)t->ctx, p, n);
}

static void index_end(tar *t) {
    apkindex *x = t->ctx;
    x->ln = 0;
    index_line(x); /* a record not followed by a blank line */
}

/* The tar stream continues across the gzip members. */
static void tar_out(void *ctx, int member, const uint8_t *p, size_t n) {
    (void)member;
    tar_feed((tar *)ctx, p, n);
}

static int db_path(char *out, const char *file) {
    char rel[PATH_MAX_GV];
    if (strlen(file) + 32 > sizeof rel) {
        return -1;
    }
    strcpy(rel, ".get-alpine/");
    strcat(rel, file);
    return under_root(out, rel);
}

static void repo_url(char *url, const char *repo, const char *file) {
    strcpy(url, mirror);
    strcat(url, "/");
    strcat(url, branch);
    strcat(url, "/");
    strcat(url, repo);
    strcat(url, "/" ALPINE_ARCH "/");
    strcat(url, file);
}

static int update_index(void) {
    char url[512], cache[PATH_MAX_GV], index[PATH_MAX_GV], tmp[PATH_MAX_GV];
    if (db_path(cache, "APKINDEX.tar.gz") || db_path(index, "index") || db_path(tmp, "index.new")) {
        return die("root path too long", NULL);
    }
    static apkindex x;
    memset(&x, 0, sizeof x);
    x.out = fopen(tmp, "w");
    if (x.out == NULL) {
        return die("cannot write ", tmp);
    }
    for (size_t r = 0; r < sizeof repos / sizeof repos[0]; r++) {
        repo_url(url, repos[r], "APKINDEX.tar.gz");
        say("fetching ", url, NULL);
        if (download(url, cache) != 0) {
            fclose(x.out);
            unlink(tmp);
            return die("download failed: ", url);
        }
        x.repo = repos[r];
        tar t;
        memset(&t, 0, sizeof t);
        t.entry = index_entry;
        t.data = index_data;
        t.end = index_end;
        t.ctx = &x;
        int members = gunzip_members(cache, NULL, tar_out, &t);
        free(t.meta);
        unlink(cache);
        if (members < 1 || t.failed) {
            fclose(x.out);
            unlink(tmp);
            return die("cannot read the index of ", repos[r]);
        }
    }
    fclose(x.out);
    if (x.packages == 0) {
        unlink(tmp);
        return die("empty repository index", NULL);
    }
    unlink(index);
    if (rename(tmp, index) != 0) {
        return die("cannot write ", index);
    }
    return 0;
}

/* Does the space-separated list `list` (provides) name `want`? Entries may
 * carry a version: "so:libjq.so.1=1.0.4". */
static int provides(const char *list, size_t len, const char *want) {
    size_t wl = strlen(want);
    const char *p = list, *end = list + len;
    while (p < end) {
        const char *e = memchr(p, ' ', (size_t)(end - p));
        if (e == NULL) {
            e = end;
        }
        size_t nl = strcspn(p, "= ");
        if (nl > (size_t)(e - p)) {
            nl = (size_t)(e - p);
        }
        if (nl == wl && memcmp(p, want, wl) == 0) {
            return 1;
        }
        p = e + 1;
    }
    return 0;
}

/* The index line of the package named `want`, or else of the first one
 * that provides it. */
static int find_package(const char *want, char *line, size_t cap) {
    char index[PATH_MAX_GV];
    db_path(index, "index");
    FILE *f = fopen(index, "r");
    if (f == NULL) {
        return -1;
    }
    static char provider[32768];
    provider[0] = '\0';
    size_t wl = strlen(want);
    int found = -1;
    while (fgets(line, (int)cap, f) != NULL) {
        const char *name = strchr(line, ' ');
        if (name == NULL) {
            continue;
        }
        name++;
        if (strncmp(name, want, wl) == 0 && name[wl] == ' ') {
            found = 0;
            break;
        }
        const char *bar = strrchr(line, '|');
        if (provider[0] == '\0' && bar != NULL && provides(bar + 1, strcspn(bar + 1, "\n"), want)) {
            copy_field(provider, sizeof provider, line, strlen(line));
        }
    }
    fclose(f);
    if (found != 0 && provider[0] != '\0') {
        copy_field(line, cap, provider, strlen(provider));
        found = 0;
    }
    if (found == 0) {
        line[strcspn(line, "\n")] = '\0';
    }
    return found;
}

/* ---- installing --------------------------------------------------------- */

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
    /* .SIGN.*, .PKGINFO, .pre-install, ...: package metadata, not files */
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
    mkdirs(path, 0);
    unlink(path);
    if (type == '2' || type == '1') {
        /* A hard link becomes a symlink to the same file (absolute in the
         * root, where `linux --root` runs). */
        char target[PATH_MAX_GV];
        if (type == '1') {
            target[0] = '/';
            copy_field(target + 1, sizeof target - 1, link, strlen(link));
        } else {
            copy_field(target, sizeof target, link, strlen(link));
        }
        if (symlink(target, path) != 0) {
            say("cannot create symlink ", path, NULL);
            u->failed = 1;
        }
        return 0;
    }
    if (type != '0' && type != '\0' && type != '7') {
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

/* Verification pass over an .apk: SHA-1 of the control member, SHA-256 of
 * the data member, and the "datahash" .PKGINFO (in the control member)
 * names. */
typedef struct {
    sha1 control;
    sha256 data;
    tar t;
    /* .PKGINFO, grown as it comes: one line per dependency and per
     * provided name makes it tens of KB for some packages (gst-plugins-bad,
     * abseil-cpp-dev), and "datahash" is its last line. */
    char *pkginfo;
    size_t pn, pcap;
    int in_pkginfo;
} verify;

static void verify_in(void *ctx, int member, const uint8_t *p, size_t n) {
    verify *v = ctx;
    if (member == 1) {
        sha1_update(&v->control, p, n);
    } else if (member == 2) {
        sha256_update(&v->data, p, n);
    }
}

static int pkginfo_entry(tar *t, char type, const char *name, const char *link, uint64_t size) {
    verify *v = t->ctx;
    (void)type;
    (void)link;
    (void)size;
    v->in_pkginfo = strcmp(name, ".PKGINFO") == 0;
    return v->in_pkginfo;
}

static void pkginfo_data(tar *t, const uint8_t *p, size_t n) {
    verify *v = t->ctx;
    if (v->pn + n + 1 > v->pcap) {
        size_t cap = v->pcap != 0 ? v->pcap : 8192;
        while (cap < v->pn + n + 1) {
            cap *= 2;
        }
        char *grown = realloc(v->pkginfo, cap);
        if (grown == NULL) {
            return; /* no datahash then: a mismatch, which refuses the package */
        }
        v->pkginfo = grown;
        v->pcap = cap;
    }
    memcpy(v->pkginfo + v->pn, p, n);
    v->pn += n;
}

static void verify_out(void *ctx, int member, const uint8_t *p, size_t n) {
    verify *v = ctx;
    if (member <= 1) {
        tar_feed(&v->t, p, n); /* signature + control: find .PKGINFO */
    }
}

static int check_apk(const char *path, const char *csum) {
    static verify v;
    memset(&v, 0, sizeof v);
    sha1_init(&v.control);
    sha256_init(&v.data);
    v.t.entry = pkginfo_entry;
    v.t.data = pkginfo_data;
    v.t.ctx = &v;
    int members = gunzip_members(path, verify_in, verify_out, &v);
    free(v.t.meta);
    int ok = 0;
    uint8_t digest[20];
    char b64[29], hex[65];
    sha1_final(&v.control, digest);
    base64_20(digest, b64);
    sha256_hex(&v.data, hex);
    if (members == 3 && strncmp(csum, "Q1", 2) == 0 && strcmp(csum + 2, b64) == 0 && v.pkginfo != NULL) {
        v.pkginfo[v.pn] = '\0';
        const char *dh = strstr(v.pkginfo, "\ndatahash = ");
        ok = dh != NULL && strncmp(dh + 12, hex, 64) == 0;
    }
    free(v.pkginfo);
    return ok ? 0 : -1;
}

static int installed(const char *name) {
    char rel[PATH_MAX_GV], path[PATH_MAX_GV];
    if (strlen(name) + 32 > sizeof rel) {
        return 0;
    }
    strcpy(rel, "pkgs/");
    strcat(rel, name);
    if (db_path(path, rel) != 0) {
        return 0;
    }
    return access(path, F_OK) == 0;
}

static void mark_installed(const char *name, const char *version) {
    char rel[PATH_MAX_GV], path[PATH_MAX_GV];
    strcpy(rel, "pkgs/");
    strcat(rel, name);
    db_path(path, rel);
    mkdirs(path, 0);
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd >= 0) {
        write(fd, version, strlen(version));
        write(fd, "\n", 1);
        close(fd);
    }
}

/* Packages and dependency names visited in this run (cycles end here). */
static char visited[1024][96];
static size_t nvisited;

static int seen(const char *name) {
    for (size_t i = 0; i < nvisited; i++) {
        if (strcmp(visited[i], name) == 0) {
            return 1;
        }
    }
    if (nvisited < sizeof visited / sizeof visited[0]) {
        copy_field(visited[nvisited++], sizeof visited[0], name, strlen(name));
    }
    return 0;
}

static int install(const char *want, int depth) {
    if (depth > 64) {
        return die("dependency chain too deep at ", want);
    }
    if (seen(want)) {
        return 0;
    }
    static char line[32768]; /* reused by the recursive calls: copy out of it */
    if (find_package(want, line, sizeof line) != 0) {
        say("not in the repositories: ", want, NULL);
        return depth == 0 ? 1 : 0; /* a missing dependency is reported, not fatal */
    }
    char repo[32], name[128], version[96], csum[64];
    const char *p = line;
    char *fields[4] = {repo, name, version, csum};
    size_t caps[4] = {sizeof repo, sizeof name, sizeof version, sizeof csum};
    for (int i = 0; i < 4; i++) {
        size_t n = strcspn(p, i < 3 ? " " : "|");
        copy_field(fields[i], caps[i], p, n);
        p += n + (p[n] != '\0');
    }
    if (installed(name) || (strcmp(name, want) != 0 && seen(name))) {
        return 0;
    }
    char *deps = strdup(p); /* "deps|provides" */
    if (deps == NULL) {
        return die("out of memory", NULL);
    }
    deps[strcspn(deps, "|")] = '\0';
    for (char *d = deps; *d != '\0';) {
        size_t n = strcspn(d, " ");
        char dep[160];
        copy_field(dep, sizeof dep, d, n);
        d += n + (d[n] == ' ');
        if (dep[0] == '!' || dep[0] == '\0') {
            continue; /* a conflict, not a dependency */
        }
        dep[strcspn(dep, "<>=~")] = '\0';
        if (install(dep, depth + 1) != 0) {
            free(deps);
            return 1;
        }
    }
    free(deps);

    char url[512], cache[PATH_MAX_GV], file[256];
    copy_field(file, sizeof file, name, strlen(name));
    strcat(file, "-");
    strcat(file, version);
    strcat(file, ".apk");
    repo_url(url, repo, file);
    if (db_path(cache, file) != 0) {
        return die("root path too long", NULL);
    }
    char what[240];
    copy_field(what, sizeof what, name, strlen(name));
    strcat(what, " ");
    strcat(what, version);
    say("", what, NULL);
    if (download(url, cache) != 0) {
        return die("download failed: ", url);
    }
    if (check_apk(cache, csum) != 0) {
        unlink(cache);
        return die("checksum mismatch: ", file);
    }
    unpack u = {-1, 0};
    tar t;
    memset(&t, 0, sizeof t);
    t.entry = unpack_entry;
    t.data = unpack_data;
    t.end = unpack_end;
    t.ctx = &u;
    int members = gunzip_members(cache, NULL, tar_out, &t);
    unpack_end(&t);
    free(t.meta);
    unlink(cache);
    if (members != 3 || t.failed || u.failed) {
        return die("cannot unpack ", file);
    }
    mark_installed(name, version);
    return 0;
}

/* A `nameserver` line per `dns=` server of the DHCP lease in /net/ndb
 * (netd's, Plan 9 style), or QEMU user networking's 10.0.2.3. */
static void write_resolv_conf(FILE *f) {
    char ndb[257];
    ssize_t n = 0;
    int fd = open("/net/ndb", O_RDONLY), any = 0;
    if (fd >= 0) {
        n = read(fd, ndb, sizeof ndb - 1);
        close(fd);
    }
    ndb[n > 0 ? n : 0] = '\0';
    for (char *p = strstr(ndb, "dns="); p != NULL; p = strstr(p, "dns=")) {
        p += 4;
        size_t len = strcspn(p, " \t\n");
        fputs("nameserver ", f);
        fwrite(p, 1, len, f);
        fputc('\n', f);
        any = 1;
    }
    if (!any) {
        fputs("nameserver 10.0.2.3\n", f);
    }
}

/* Alpine's packages depend on no font: a root with just a GTK program (a
 * browser) has fontconfig and nothing under /usr/share/fonts, and GTK, laying
 * its text out without one, asks X for a window wider than X allows and dies
 * with an X error (BadAlloc). Say so when the install is done. */
static void font_hint(void) {
    char path[PATH_MAX_GV];
    if (under_root(path, "usr/lib/libfontconfig.so.1") != 0 || access(path, F_OK) != 0) {
        return;
    }
    int any = 0;
    if (under_root(path, "usr/share/fonts") == 0) {
        DIR *d = opendir(path);
        if (d != NULL) {
            struct dirent *e;
            while (!any && (e = readdir(d)) != NULL) {
                any = strcmp(e->d_name, ".") != 0 && strcmp(e->d_name, "..") != 0;
            }
            closedir(d);
        }
    }
    if (!any) {
        say("no fonts in ", pkg_root,
            "/usr/share/fonts: a program that draws text (GTK, a browser) needs one: get-alpine font-dejavu");
    }
}

/* `linux --root ROOT apk add --initdb [-U] BASE... PACKAGE...`: apk's exit
 * status, 127 when it did not run. --initdb makes the database on the first
 * run and is nothing afterwards; the base set is in every `add`, so apk's
 * world lists it from the start and an already installed package costs
 * nothing. apk is quiet about progress: the console is a serial line as
 * often as not. */
static int run_apk(int update, int npkgs, char **pkgs) {
    char **args = calloc(10 + NBASE + (size_t)npkgs, sizeof *args);
    if (args == NULL) {
        return die("out of memory", NULL);
    }
    size_t n = 0;
    args[n++] = "linux";
    args[n++] = "--root";
    args[n++] = (char *)pkg_root;
    args[n++] = "apk";
    args[n++] = "add";
    args[n++] = "--initdb";
    args[n++] = "--no-progress";
    if (update) {
        args[n++] = "--update-cache";
    }
    for (size_t k = 0; k < NBASE; k++) {
        args[n++] = (char *)base[k];
    }
    for (int k = 0; k < npkgs; k++) {
        args[n++] = pkgs[k];
    }
    args[n] = NULL;
    pid_t pid = fork();
    if (pid < 0) {
        free(args);
        return die("cannot fork", NULL);
    }
    if (pid == 0) {
        execvp("linux", args);
        _exit(127);
    }
    free(args);
    int status = 0;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status)) {
        return die("apk did not finish", NULL);
    }
    if (WEXITSTATUS(status) == 127) {
        return die("cannot run the linux launcher: is the module loaded (insmod /lib/modules/linux)?", NULL);
    }
    return WEXITSTATUS(status);
}

int main(int argc, char **argv) {
    int update = 0, native = 0, i = 1;
    pkg_prog = "get-alpine";
    pkg_root = data_mounted() ? "/data/alpine" : "/tmp/alpine";
    for (; i < argc && argv[i][0] == '-'; i++) {
        if (strcmp(argv[i], "-r") == 0 && i + 1 < argc) {
            pkg_root = argv[++i];
        } else if (strcmp(argv[i], "-u") == 0) {
            update = 1;
        } else if (strcmp(argv[i], "-n") == 0) {
            native = 1;
        } else {
            break;
        }
    }
    if (native && i >= argc && !update) {
        fputs("usage: get-alpine [-r ROOT] [-u] [-n] [PACKAGE...]\n"
              "  Install Alpine Linux (" ALPINE_ARCH ") packages and their dependencies\n"
              "  into ROOT (default /data/alpine with /data mounted, else /tmp/alpine):\n"
              "  the root gets apk, which installs them (scripts included) and\n"
              "  everything later: linux --root ROOT apk add|del|upgrade ...\n"
              "  (no PACKAGE: just that); run them with: linux --root ROOT PROGRAM\n"
              "  -u  refresh the repository indexes\n"
              "  -n  no apk: fetch, check and unpack the packages here, without\n"
              "      their install scripts (the Linux layer need not be loaded)\n",
              stderr);
        return 2;
    }
    const char *m = getenv("ALPINE_MIRROR"), *b = getenv("ALPINE_BRANCH");
    copy_field(mirror, sizeof mirror, m != NULL ? m : DEFAULT_MIRROR, sizeof mirror);
    copy_field(branch, sizeof branch, b != NULL ? b : DEFAULT_BRANCH, sizeof branch);
    if (pkg_root[0] != '/' || strlen(pkg_root) > 100) {
        return die("ROOT must be an absolute path of at most 100 bytes: ", pkg_root);
    }
    static const char *dirs[] = {"dev", "proc", "net", "tmp", "etc/apk", ".get-alpine/pkgs"};
    char path[PATH_MAX_GV];
    mkdirs(pkg_root, 1);
    for (size_t k = 0; k < sizeof dirs / sizeof dirs[0]; k++) {
        if (under_root(path, dirs[k]) == 0) {
            mkdirs(path, 1);
        }
    }
    /* The DNS servers the system uses, for musl: the `dns=` lines of the
     * DHCP lease (/net/ndb), QEMU user networking's when it names none. */
    if (under_root(path, "etc/resolv.conf") == 0 && access(path, F_OK) != 0) {
        FILE *f = fopen(path, "w");
        if (f != NULL) {
            write_resolv_conf(f);
            fclose(f);
        }
    }
    /* The repositories apk reads, the mirror and branch of this run. */
    if (under_root(path, "etc/apk/repositories") == 0 && access(path, F_OK) != 0) {
        FILE *f = fopen(path, "w");
        if (f != NULL) {
            for (size_t r = 0; r < sizeof repos / sizeof repos[0]; r++) {
                fputs(mirror, f);
                fputc('/', f);
                fputs(branch, f);
                fputc('/', f);
                fputs(repos[r], f);
                fputc('\n', f);
            }
            fclose(f);
        }
    }
    char index[PATH_MAX_GV];
    db_path(index, "index");
    int bootstrapped = under_root(path, "sbin/apk") == 0 && access(path, X_OK) == 0;
    if ((native || !bootstrapped) && (update || access(index, F_OK) != 0) && update_index() != 0) {
        return 1;
    }
    if (native) {
        for (; i < argc; i++) {
            if (install(argv[i], 0) != 0) {
                return 1;
            }
        }
        if (!installed("ca-certificates") && install("ca-certificates-bundle", 1) != 0) {
            return 1;
        }
    } else {
        if (!bootstrapped) {
            say("bootstrapping apk in ", pkg_root, NULL);
            for (size_t k = 0; k < NBASE; k++) {
                if (install(base[k], 0) != 0) {
                    return 1;
                }
            }
        }
        if (run_apk(update, argc - i, argv + i) != 0) {
            return die("apk failed in ", pkg_root);
        }
    }
    font_hint();
    say("done; run with: linux --root ", pkg_root, " PROGRAM");
    return 0;
}
