/*
 * get-alpine [-r ROOT] [-u] PACKAGE...
 *
 * Download Alpine Linux packages, with their run-time dependencies, into a
 * Linux root (default /tmp/alpine), to run them with
 *     linux --root ROOT PROGRAM [ARG...]
 * Part of the optional Linux compatibility layer (docs/linux-compat.md):
 * nothing from Alpine is in the image, everything is fetched at run time.
 *
 * The repository indexes (main and community: APKINDEX.tar.gz) are
 * downloaded once and reduced to ROOT/var/lib/get-alpine/index, one line
 * per package: repository, name, version, checksum, dependencies, provides.
 * -u refreshes them. Dependencies ("so:libonig.so.5", "cmd:sh", names) are
 * resolved through package names and provides.
 *
 * A package (.apk) is three concatenated gzip streams: signature, control
 * (.PKGINFO) and data. The index's checksum is the SHA-1 of the control
 * stream, whose .PKGINFO holds the SHA-256 of the data stream ("datahash"):
 * both are checked before anything is unpacked into ROOT. (The repository
 * signature is not checked; the index comes over HTTPS.) Install scripts
 * are not run.
 *
 * Alpine has x86_64, aarch64 and riscv64 repositories. ALPINE_MIRROR
 * overrides https://dl-cdn.alpinelinux.org/alpine and ALPINE_BRANCH
 * latest-stable.
 *
 * Uses fputs, not printf: newlib's printf needs extra soft-float helpers
 * on aarch64 and riscv64.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#include "zlib.h"

#if defined(__x86_64__)
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
#define PATH_MAX_GV 256

static const char *root = "/tmp/alpine";
static char mirror[160], branch[64];
static const char *const repos[] = {"main", "community"};

/* ---- messages ----------------------------------------------------------- */

static void say(const char *a, const char *b, const char *c) {
    fputs(a, stderr);
    if (b != NULL) {
        fputs(b, stderr);
    }
    if (c != NULL) {
        fputs(c, stderr);
    }
    fputs("\n", stderr);
}

static int die(const char *a, const char *b) {
    say("get-alpine: ", a, b);
    return 1;
}

/* ---- paths -------------------------------------------------------------- */

/* out = root + "/" + rel (rel without a leading '/'). */
static int under_root(char *out, const char *rel) {
    size_t r = strlen(root), n = strlen(rel);
    if (r + 1 + n + 1 > PATH_MAX_GV) {
        return -1;
    }
    memcpy(out, root, r);
    out[r] = '/';
    memcpy(out + r + 1, rel, n + 1);
    return 0;
}

/* mkdir -p of path's directories (and path itself if `self`). */
static void mkdirs(const char *path, int self) {
    char buf[PATH_MAX_GV];
    size_t n = strlen(path);
    if (n >= sizeof buf) {
        return;
    }
    memcpy(buf, path, n + 1);
    for (size_t i = 1; i <= n; i++) {
        if (buf[i] == '/' || (self && buf[i] == '\0')) {
            char c = buf[i];
            buf[i] = '\0';
            mkdir(buf, 0755); /* EEXIST (also for a symlinked dir) is fine */
            buf[i] = c;
        }
    }
}

/* ---- running curl ------------------------------------------------------- */

static int download_once(const char *url, const char *dest) {
    pid_t pid = fork();
    if (pid < 0) {
        return -1;
    }
    if (pid == 0) {
        char *argv[] = {"curl", "-fsS", "--connect-timeout", "30", "--max-time", "900",
                        "-o", (char *)dest, (char *)url, NULL};
        execvp("curl", argv);
        _exit(127);
    }
    int status = 0;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        unlink(dest);
        return -1;
    }
    return 0;
}

/* A transient connect failure should not fail the whole install. */
static int download(const char *url, const char *dest) {
    for (int attempt = 1;; attempt++) {
        if (download_once(url, dest) == 0) {
            return 0;
        }
        if (attempt == 3) {
            return -1;
        }
        say("get-alpine: retrying ", url, NULL);
        sleep(2);
    }
}

/* ---- sha256 ------------------------------------------------------------- */

typedef struct {
    uint32_t h[8];
    uint64_t len;
    uint8_t buf[64];
    size_t n;
} sha256;

static const uint32_t K[64] = {
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
};

#define ROR(x, n) (((x) >> (n)) | ((x) << (32 - (n))))

static void sha256_block(sha256 *s, const uint8_t *p) {
    uint32_t w[64], a, b, c, d, e, f, g, h;
    for (int i = 0; i < 16; i++) {
        w[i] = (uint32_t)p[4 * i] << 24 | (uint32_t)p[4 * i + 1] << 16 | (uint32_t)p[4 * i + 2] << 8 |
               p[4 * i + 3];
    }
    for (int i = 16; i < 64; i++) {
        uint32_t s0 = ROR(w[i - 15], 7) ^ ROR(w[i - 15], 18) ^ (w[i - 15] >> 3);
        uint32_t s1 = ROR(w[i - 2], 17) ^ ROR(w[i - 2], 19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    a = s->h[0], b = s->h[1], c = s->h[2], d = s->h[3];
    e = s->h[4], f = s->h[5], g = s->h[6], h = s->h[7];
    for (int i = 0; i < 64; i++) {
        uint32_t t1 = h + (ROR(e, 6) ^ ROR(e, 11) ^ ROR(e, 25)) + ((e & f) ^ (~e & g)) + K[i] + w[i];
        uint32_t t2 = (ROR(a, 2) ^ ROR(a, 13) ^ ROR(a, 22)) + ((a & b) ^ (a & c) ^ (b & c));
        h = g, g = f, f = e, e = d + t1, d = c, c = b, b = a, a = t1 + t2;
    }
    s->h[0] += a, s->h[1] += b, s->h[2] += c, s->h[3] += d;
    s->h[4] += e, s->h[5] += f, s->h[6] += g, s->h[7] += h;
}

static void sha256_init(sha256 *s) {
    static const uint32_t iv[8] = {0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                                   0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
    memcpy(s->h, iv, sizeof iv);
    s->len = 0;
    s->n = 0;
}

static void sha256_update(sha256 *s, const uint8_t *p, size_t n) {
    s->len += n;
    while (n > 0) {
        size_t k = 64 - s->n < n ? 64 - s->n : n;
        memcpy(s->buf + s->n, p, k);
        s->n += k, p += k, n -= k;
        if (s->n == 64) {
            sha256_block(s, s->buf);
            s->n = 0;
        }
    }
}

/* Lowercase hex digest into hex[65]. */
static void sha256_hex(sha256 *s, char *hex) {
    uint64_t bits = s->len * 8;
    uint8_t pad = 0x80, zero = 0, lenb[8];
    sha256_update(s, &pad, 1);
    while (s->n != 56) {
        sha256_update(s, &zero, 1);
    }
    for (int i = 0; i < 8; i++) {
        lenb[i] = (uint8_t)(bits >> (56 - 8 * i));
    }
    sha256_update(s, lenb, 8);
    for (int i = 0; i < 32; i++) {
        uint8_t b = (uint8_t)(s->h[i / 4] >> (24 - 8 * (i % 4)));
        hex[2 * i] = "0123456789abcdef"[b >> 4];
        hex[2 * i + 1] = "0123456789abcdef"[b & 15];
    }
    hex[64] = '\0';
}

/* ---- tar (ustar + pax/GNU long names) ----------------------------------- */

typedef struct tar tar;
struct tar {
    /* An entry starts: return 1 to receive its data through `data`. */
    int (*entry)(tar *t, char type, const char *name, const char *link, uint64_t size);
    void (*data)(tar *t, const uint8_t *p, size_t n);
    void (*end)(tar *t); /* after the last data of a received entry */
    void *ctx;
    /* parser state */
    uint8_t hdr[512];
    size_t hn;
    uint64_t left, pad;
    int want;
    char kind; /* 'x' / 'L' / 'K': collecting a long name, else 0 */
    char *meta;
    size_t metan;
    char longname[PATH_MAX_GV], longlink[PATH_MAX_GV];
    int failed;
};

static uint64_t octal(const uint8_t *p, size_t n) {
    uint64_t v = 0;
    for (size_t i = 0; i < n && p[i] >= '0' && p[i] <= '7'; i++) {
        v = v * 8 + (uint64_t)(p[i] - '0');
    }
    return v;
}

static void copy_field(char *out, size_t cap, const char *s, size_t n) {
    size_t k = 0;
    while (k < n && s[k] != '\0' && k + 1 < cap) {
        out[k] = s[k];
        k++;
    }
    out[k] = '\0';
}

/* A pax extended header: records "LEN key=value\n". */
static void pax(tar *t) {
    size_t i = 0;
    while (i < t->metan) {
        size_t len = 0, j = i;
        while (j < t->metan && t->meta[j] >= '0' && t->meta[j] <= '9') {
            len = len * 10 + (size_t)(t->meta[j++] - '0');
        }
        if (len == 0 || i + len > t->metan || j >= t->metan) {
            return;
        }
        const char *kv = t->meta + j + 1, *end = t->meta + i + len - 1; /* drop '\n' */
        const char *eq = memchr(kv, '=', (size_t)(end - kv));
        if (eq != NULL) {
            size_t kl = (size_t)(eq - kv), vl = (size_t)(end - eq - 1);
            if (kl == 4 && memcmp(kv, "path", 4) == 0) {
                copy_field(t->longname, sizeof t->longname, eq + 1, vl);
            } else if (kl == 8 && memcmp(kv, "linkpath", 8) == 0) {
                copy_field(t->longlink, sizeof t->longlink, eq + 1, vl);
            }
        }
        i += len;
    }
}

static void tar_header(tar *t) {
    const uint8_t *h = t->hdr;
    int zero = 1;
    for (int i = 0; i < 512; i++) {
        if (h[i] != 0) {
            zero = 0;
            break;
        }
    }
    if (zero) {
        return; /* end-of-archive blocks */
    }
    uint64_t size = octal(h + 124, 12);
    char type = (char)h[156];
    t->left = size;
    t->pad = (512 - size % 512) % 512;
    t->want = 0;
    t->kind = 0;
    if (type == 'x' || type == 'L' || type == 'K') {
        if (size > 65536) {
            t->failed = 1;
            return;
        }
        free(t->meta);
        t->meta = malloc(size + 1);
        t->metan = 0;
        t->kind = type;
        return;
    }
    if (type == 'g') {
        return;
    }
    char name[PATH_MAX_GV], link[PATH_MAX_GV];
    if (t->longname[0] != '\0') {
        copy_field(name, sizeof name, t->longname, sizeof t->longname);
    } else {
        char base[101], prefix[156];
        copy_field(base, sizeof base, (const char *)h, 100);
        copy_field(prefix, sizeof prefix, (const char *)h + 345, 155);
        if (prefix[0] != '\0' && strlen(prefix) + 1 + strlen(base) < sizeof name) {
            strcpy(name, prefix);
            strcat(name, "/");
            strcat(name, base);
        } else {
            copy_field(name, sizeof name, base, sizeof base);
        }
    }
    if (t->longlink[0] != '\0') {
        copy_field(link, sizeof link, t->longlink, sizeof t->longlink);
    } else {
        copy_field(link, sizeof link, (const char *)h + 157, 100);
    }
    t->longname[0] = t->longlink[0] = '\0';
    t->want = t->entry(t, type, name, link, size);
    if (t->want && size == 0 && t->end != NULL) {
        t->end(t);
    }
}

static void tar_feed(tar *t, const uint8_t *p, size_t n) {
    while (n > 0 && !t->failed) {
        if (t->left > 0) {
            size_t k = t->left < n ? (size_t)t->left : n;
            if (t->kind != 0) {
                memcpy(t->meta + t->metan, p, k);
                t->metan += k;
            } else if (t->want) {
                t->data(t, p, k);
            }
            t->left -= k, p += k, n -= k;
            if (t->left == 0) {
                if (t->kind == 'x') {
                    pax(t);
                } else if (t->kind == 'L' || t->kind == 'K') {
                    char *dst = t->kind == 'L' ? t->longname : t->longlink;
                    copy_field(dst, PATH_MAX_GV, t->meta, t->metan);
                } else if (t->want && t->end != NULL) {
                    t->end(t);
                }
                t->kind = 0;
            }
        } else if (t->pad > 0) {
            size_t k = t->pad < n ? (size_t)t->pad : n;
            t->pad -= k, p += k, n -= k;
        } else {
            size_t k = 512 - t->hn < n ? 512 - t->hn : n;
            memcpy(t->hdr + t->hn, p, k);
            t->hn += k, p += k, n -= k;
            if (t->hn == 512) {
                t->hn = 0;
                tar_header(t);
            }
        }
    }
}

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

/* ---- concatenated gzip streams ------------------------------------------ */

/* Inflate the gzip members of the file at `path` one after the other. The
 * compressed bytes of member i go to in(i), the decompressed ones to out(i);
 * either may be NULL. Returns the number of members, or -1. */
static int gunzip_members(const char *path, void (*in)(void *ctx, int member, const uint8_t *p, size_t n),
                          void (*out)(void *ctx, int member, const uint8_t *p, size_t n), void *ctx) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return -1;
    }
    static uint8_t ibuf[16384], obuf[32768];
    z_stream zs;
    memset(&zs, 0, sizeof zs);
    if (inflateInit2(&zs, 16 + MAX_WBITS) != Z_OK) {
        close(fd);
        return -1;
    }
    int member = 0, rc = Z_OK, open_member = 0;
    ssize_t n;
    while ((n = read(fd, ibuf, sizeof ibuf)) > 0) {
        zs.next_in = ibuf;
        zs.avail_in = (uInt)n;
        while (zs.avail_in > 0) {
            const uint8_t *before = zs.next_in;
            zs.next_out = obuf;
            zs.avail_out = sizeof obuf;
            open_member = 1;
            rc = inflate(&zs, Z_NO_FLUSH);
            if (rc != Z_OK && rc != Z_STREAM_END && rc != Z_BUF_ERROR) {
                inflateEnd(&zs);
                close(fd);
                return -1;
            }
            if (in != NULL) {
                in(ctx, member, before, (size_t)(zs.next_in - before));
            }
            if (out != NULL && zs.next_out != obuf) {
                out(ctx, member, obuf, (size_t)(zs.next_out - obuf));
            }
            if (rc == Z_STREAM_END) {
                member++;
                open_member = 0;
                inflateReset(&zs);
            } else if (rc == Z_BUF_ERROR && zs.avail_in > 0) {
                break; /* no progress possible: corrupt */
            }
        }
    }
    /* Drain output a member still holds once the input is all in. */
    while (open_member) {
        zs.next_out = obuf;
        zs.avail_out = sizeof obuf;
        rc = inflate(&zs, Z_NO_FLUSH);
        if (out != NULL && zs.next_out != obuf) {
            out(ctx, member, obuf, (size_t)(zs.next_out - obuf));
        }
        if (rc == Z_STREAM_END) {
            member++;
            open_member = 0;
        } else if (zs.next_out == obuf) {
            break; /* truncated */
        }
    }
    inflateEnd(&zs);
    close(fd);
    return n == 0 && !open_member ? member : -1;
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
    strcpy(rel, "var/lib/get-alpine/");
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
        say("get-alpine: fetching ", url, NULL);
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
            say("get-alpine: cannot create symlink ", path, NULL);
            u->failed = 1;
        }
        return 0;
    }
    if (type != '0' && type != '\0' && type != '7') {
        return 0;
    }
    u->fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0755);
    if (u->fd < 0) {
        say("get-alpine: cannot write ", path, NULL);
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
    char pkginfo[8192];
    size_t pn;
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
    size_t k = n < sizeof v->pkginfo - 1 - v->pn ? n : sizeof v->pkginfo - 1 - v->pn;
    memcpy(v->pkginfo + v->pn, p, k);
    v->pn += k;
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
    if (members != 3) {
        return -1;
    }
    uint8_t digest[20];
    char b64[29], hex[65];
    sha1_final(&v.control, digest);
    base64_20(digest, b64);
    if (strncmp(csum, "Q1", 2) != 0 || strcmp(csum + 2, b64) != 0) {
        return -1;
    }
    v.pkginfo[v.pn] = '\0';
    const char *dh = strstr(v.pkginfo, "\ndatahash = ");
    if (dh == NULL) {
        return -1;
    }
    sha256_hex(&v.data, hex);
    return strncmp(dh + 12, hex, 64) == 0 ? 0 : -1;
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
        say("get-alpine: not in the repositories: ", want, NULL);
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
    say("get-alpine: ", what, NULL);
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

int main(int argc, char **argv) {
    int update = 0, i = 1;
    for (; i < argc && argv[i][0] == '-'; i++) {
        if (strcmp(argv[i], "-r") == 0 && i + 1 < argc) {
            root = argv[++i];
        } else if (strcmp(argv[i], "-u") == 0) {
            update = 1;
        } else {
            break;
        }
    }
    if (i >= argc && !update) {
        fputs("usage: get-alpine [-r ROOT] [-u] PACKAGE...\n"
              "  Install Alpine Linux (" ALPINE_ARCH ") packages and their dependencies\n"
              "  into ROOT (default /tmp/alpine); run them with: linux --root ROOT PROGRAM\n"
              "  -u  refresh the repository indexes\n",
              stderr);
        return 2;
    }
    const char *m = getenv("ALPINE_MIRROR"), *b = getenv("ALPINE_BRANCH");
    copy_field(mirror, sizeof mirror, m != NULL ? m : DEFAULT_MIRROR, sizeof mirror);
    copy_field(branch, sizeof branch, b != NULL ? b : DEFAULT_BRANCH, sizeof branch);
    if (root[0] != '/' || strlen(root) > 100) {
        return die("ROOT must be an absolute path of at most 100 bytes: ", root);
    }
    static const char *dirs[] = {"dev", "proc", "tmp", "etc", "var/lib/get-alpine/pkgs"};
    char path[PATH_MAX_GV];
    mkdirs(root, 1);
    for (size_t k = 0; k < sizeof dirs / sizeof dirs[0]; k++) {
        if (under_root(path, dirs[k]) == 0) {
            mkdirs(path, 1);
        }
    }
    /* The resolver the system uses (QEMU user networking's DNS), for musl. */
    if (under_root(path, "etc/resolv.conf") == 0 && access(path, F_OK) != 0) {
        FILE *f = fopen(path, "w");
        if (f != NULL) {
            fputs("nameserver 10.0.2.3\n", f);
            fclose(f);
        }
    }
    char index[PATH_MAX_GV];
    db_path(index, "index");
    if ((update || access(index, F_OK) != 0) && update_index() != 0) {
        return 1;
    }
    for (; i < argc; i++) {
        if (install(argv[i], 0) != 0) {
            return 1;
        }
    }
    say("get-alpine: done; run with: linux --root ", root, " PROGRAM");
    return 0;
}
