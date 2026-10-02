/*
 * get-void [-r ROOT] [-u] PACKAGE...
 *
 * Download Void Linux (musl) packages, with their run-time dependencies,
 * into a Linux root (default /tmp/void), to run them with
 *     linux --root ROOT PROGRAM [ARG...]
 * Part of the optional Linux compatibility layer (docs/linux-compat.md):
 * nothing from Void is in the image, everything is fetched at run time.
 *
 * The repository index (<arch>-repodata, a zstd-compressed tar holding
 * index.plist) is downloaded once and reduced to ROOT/var/db/get-void/index
 * (one line per package: pkgver, sha256, run_depends); -u refreshes it.
 * Each package is downloaded with curl, checked against the index's sha256,
 * and unpacked (zstd + tar) into ROOT. INSTALL/REMOVE scripts are not run.
 *
 * Void has x86_64 and aarch64 musl repositories, no riscv64 one.
 * GET_VOID_MIRROR overrides https://repo-default.voidlinux.org.
 *
 * Uses fputs, not printf: newlib's printf needs extra soft-float helpers
 * on aarch64.
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

#include "zstd.h"

#if defined(__x86_64__)
#define VOID_ARCH "x86_64-musl"
#define VOID_REPO "/current/musl"
#elif defined(__aarch64__)
#define VOID_ARCH "aarch64-musl"
#define VOID_REPO "/current/aarch64"
#else
#error "Void Linux has no musl repository for this architecture"
#endif

#define DEFAULT_MIRROR "https://repo-default.voidlinux.org"
#define PATH_MAX_GV 256

static const char *root = "/tmp/void";
static char mirror[256];

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
    say("get-void: ", a, b);
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

static int download(const char *url, const char *dest) {
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

static int file_sha256(const char *path, char *hex) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return -1;
    }
    static uint8_t buf[32768];
    sha256 s;
    sha256_init(&s);
    ssize_t n;
    while ((n = read(fd, buf, sizeof buf)) > 0) {
        sha256_update(&s, buf, (size_t)n);
    }
    close(fd);
    if (n < 0) {
        return -1;
    }
    sha256_hex(&s, hex);
    return 0;
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

/* Decompress the zstd file at `path` into the tar parser. */
static int unzstd_tar(const char *path, tar *t) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return -1;
    }
    ZSTD_DStream *z = ZSTD_createDStream();
    size_t incap = ZSTD_DStreamInSize(), outcap = ZSTD_DStreamOutSize();
    uint8_t *in = malloc(incap), *out = malloc(outcap);
    int rc = -1;
    if (z == NULL || in == NULL || out == NULL) {
        goto done;
    }
    ZSTD_initDStream(z);
    size_t last = 0;
    ssize_t n;
    while ((n = read(fd, in, incap)) > 0) {
        ZSTD_inBuffer ib = {in, (size_t)n, 0};
        while (ib.pos < ib.size) {
            ZSTD_outBuffer ob = {out, outcap, 0};
            last = ZSTD_decompressStream(z, &ob, &ib);
            if (ZSTD_isError(last)) {
                say("get-void: zstd: ", ZSTD_getErrorName(last), NULL);
                goto done;
            }
            tar_feed(t, out, ob.pos);
            if (t->failed) {
                goto done;
            }
        }
    }
    /* Flush what the decoder still holds; 0 means the frame is complete
     * (calling again would start on a next frame). */
    while (last != 0 && !ZSTD_isError(last)) {
        ZSTD_inBuffer ib = {in, 0, 0};
        ZSTD_outBuffer ob = {out, outcap, 0};
        last = ZSTD_decompressStream(z, &ob, &ib);
        if (ZSTD_isError(last) || ob.pos == 0) {
            break; /* an error, or a truncated file */
        }
        tar_feed(t, out, ob.pos);
    }
    rc = n == 0 && last == 0 && !t->failed ? 0 : -1;
done:
    ZSTD_freeDStream(z);
    free(in);
    free(out);
    free(t->meta);
    t->meta = NULL;
    close(fd);
    return rc;
}

/* ---- the repository index ---------------------------------------------- */

/* Streaming reader of index.plist: <dict> of package name -> <dict>. It
 * writes "pkgver sha256 dep...\n" per package to `out`. */
typedef struct {
    FILE *out;
    int in_tag, tag_done;
    char tag[32];
    size_t tn;
    char text[512];
    size_t xn;
    int dict, array; /* depths (arrays only counted inside a package dict) */
    char key[32];
    char pkgver[128], sha[72];
    char deps[4096];
    size_t dn;
    long packages;
} plist;

static void plist_text_decoded(plist *p, char *out, size_t cap) {
    size_t o = 0;
    for (size_t i = 0; i < p->xn && o + 1 < cap; i++) {
        static const struct { const char *e; char c; } ents[] = {
            {"&lt;", '<'}, {"&gt;", '>'}, {"&amp;", '&'}, {"&quot;", '"'}, {"&apos;", '\''}};
        char c = p->text[i];
        if (c == '&') {
            for (size_t k = 0; k < sizeof ents / sizeof ents[0]; k++) {
                size_t l = strlen(ents[k].e);
                if (i + l <= p->xn && memcmp(p->text + i, ents[k].e, l) == 0) {
                    c = ents[k].c;
                    i += l - 1;
                    break;
                }
            }
        }
        out[o++] = c;
    }
    out[o] = '\0';
}

static void plist_tag(plist *p) {
    p->tag[p->tn] = '\0';
    const char *t = p->tag;
    int closing = t[0] == '/';
    if (closing) {
        t++;
    }
    if (t[0] == '?' || t[0] == '!' || (p->tn > 0 && p->tag[p->tn - 1] == '/')) {
        p->xn = 0;
        return; /* declarations and empty elements (<true/>) */
    }
    char text[512];
    if (strcmp(t, "dict") == 0) {
        if (!closing) {
            p->dict++;
            if (p->dict == 2) {
                p->pkgver[0] = p->sha[0] = '\0';
                p->dn = 0;
                p->array = 0;
            }
        } else {
            if (p->dict == 2 && p->pkgver[0] != '\0' && p->sha[0] != '\0') {
                p->deps[p->dn] = '\0';
                fputs(p->pkgver, p->out);
                fputs(" ", p->out);
                fputs(p->sha, p->out);
                fputs(p->deps, p->out);
                fputs("\n", p->out);
                p->packages++;
            }
            p->dict--;
        }
    } else if (p->dict == 2 && strcmp(t, "array") == 0) {
        p->array += closing ? -1 : 1;
    } else if (p->dict == 2 && closing && strcmp(t, "key") == 0 && p->array == 0) {
        plist_text_decoded(p, text, sizeof text);
        copy_field(p->key, sizeof p->key, text, sizeof text);
    } else if (p->dict == 2 && closing && strcmp(t, "string") == 0) {
        plist_text_decoded(p, text, sizeof text);
        if (p->array == 0 && strcmp(p->key, "pkgver") == 0) {
            copy_field(p->pkgver, sizeof p->pkgver, text, sizeof text);
        } else if (p->array == 0 && strcmp(p->key, "filename-sha256") == 0) {
            copy_field(p->sha, sizeof p->sha, text, sizeof text);
        } else if (p->array == 1 && strcmp(p->key, "run_depends") == 0) {
            size_t l = strlen(text);
            if (p->dn + 1 + l + 1 < sizeof p->deps && strchr(text, ' ') == NULL) {
                p->deps[p->dn++] = ' ';
                memcpy(p->deps + p->dn, text, l);
                p->dn += l;
            }
        }
    }
    p->xn = 0;
}

static void plist_feed(plist *p, const uint8_t *s, size_t n) {
    for (size_t i = 0; i < n; i++) {
        char c = (char)s[i];
        if (p->in_tag) {
            if (c == '>') {
                p->in_tag = 0;
                plist_tag(p);
            } else if (c == ' ' || c == '\t' || c == '\n') {
                p->tag_done = 1; /* attributes: keep the name only */
            } else if (!p->tag_done && p->tn + 1 < sizeof p->tag) {
                p->tag[p->tn++] = c;
            }
        } else if (c == '<') {
            p->in_tag = 1;
            p->tag_done = 0;
            p->tn = 0;
        } else if (p->xn + 1 < sizeof p->text) {
            p->text[p->xn++] = c;
        }
    }
}

static int index_entry(tar *t, char type, const char *name, const char *link, uint64_t size) {
    (void)link;
    (void)size;
    return (type == '0' || type == '\0') &&
           (strcmp(name, "index.plist") == 0 || strcmp(name, "./index.plist") == 0);
}

static void index_data(tar *t, const uint8_t *p, size_t n) {
    plist_feed((plist *)t->ctx, p, n);
}

static int db_path(char *out, const char *file) {
    char rel[PATH_MAX_GV];
    if (strlen(file) + 32 > sizeof rel) {
        return -1;
    }
    strcpy(rel, "var/db/get-void/");
    strcat(rel, file);
    return under_root(out, rel);
}

static int update_index(void) {
    char url[512], cache[PATH_MAX_GV], index[PATH_MAX_GV], tmp[PATH_MAX_GV];
    if (db_path(cache, VOID_ARCH "-repodata") || db_path(index, "index") || db_path(tmp, "index.new")) {
        return die("root path too long", NULL);
    }
    strcpy(url, mirror);
    strcat(url, VOID_REPO "/" VOID_ARCH "-repodata");
    say("get-void: fetching ", url, NULL);
    if (download(url, cache) != 0) {
        return die("download failed: ", url);
    }
    static plist p;
    memset(&p, 0, sizeof p);
    p.out = fopen(tmp, "w");
    if (p.out == NULL) {
        return die("cannot write ", tmp);
    }
    tar t;
    memset(&t, 0, sizeof t);
    t.entry = index_entry;
    t.data = index_data;
    t.ctx = &p;
    int rc = unzstd_tar(cache, &t);
    fclose(p.out);
    unlink(cache);
    if (rc != 0 || p.packages == 0) {
        unlink(tmp);
        return die("cannot read the repository index", NULL);
    }
    unlink(index);
    if (rename(tmp, index) != 0) {
        return die("cannot write ", index);
    }
    return 0;
}

/* The index line of package `name` (the pkgver up to its last '-'). */
static int find_package(const char *name, char *line, size_t cap) {
    char index[PATH_MAX_GV];
    db_path(index, "index");
    FILE *f = fopen(index, "r");
    if (f == NULL) {
        return -1;
    }
    size_t nl = strlen(name);
    int found = -1;
    while (fgets(line, (int)cap, f) != NULL) {
        if (strncmp(line, name, nl) == 0 && line[nl] == '-') {
            const char *ver = line + nl + 1, *sp = strchr(ver, ' ');
            /* the version part holds no further '-' */
            const char *dash = memchr(ver, '-', sp != NULL ? (size_t)(sp - ver) : strlen(ver));
            if (dash == NULL) {
                found = 0;
                break;
            }
        }
    }
    fclose(f);
    if (found == 0) {
        line[strcspn(line, "\n")] = '\0';
    }
    return found;
}

/* ---- installing --------------------------------------------------------- */

static int skip_member(const char *rel) {
    static const char *meta[] = {"props.plist", "files.plist", "INSTALL", "REMOVE", "INSTALL.msg",
                                 "REMOVE.msg"};
    for (size_t i = 0; i < sizeof meta / sizeof meta[0]; i++) {
        if (strcmp(rel, meta[i]) == 0) {
            return 1;
        }
    }
    /* never write outside the root */
    if (strcmp(rel, "..") == 0 || strncmp(rel, "../", 3) == 0 || strstr(rel, "/../") != NULL) {
        return 1;
    }
    size_t n = strlen(rel);
    return n >= 3 && strcmp(rel + n - 3, "/..") == 0;
}

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
    if (name[0] == '\0' || skip_member(name)) {
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
            const char *l = link;
            while (l[0] == '.' && l[1] == '/') {
                l += 2;
            }
            target[0] = '/';
            copy_field(target + 1, sizeof target - 1, l, strlen(l));
        } else {
            copy_field(target, sizeof target, link, strlen(link));
        }
        if (symlink(target, path) != 0) {
            say("get-void: cannot create symlink ", path, NULL);
            u->failed = 1;
        }
        return 0;
    }
    if (type != '0' && type != '\0' && type != '7') {
        return 0; /* devices, fifos: not in packages */
    }
    u->fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0755);
    if (u->fd < 0) {
        say("get-void: cannot write ", path, NULL);
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

static void mark_installed(const char *name, const char *pkgver) {
    char rel[PATH_MAX_GV], path[PATH_MAX_GV];
    strcpy(rel, "pkgs/");
    strcat(rel, name);
    db_path(path, rel);
    mkdirs(path, 0);
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd >= 0) {
        write(fd, pkgver, strlen(pkgver));
        write(fd, "\n", 1);
        close(fd);
    }
}

/* The package name in a dependency pattern: "musl>=1.2.5_1" -> "musl". */
static void dep_name(const char *dep, char *out, size_t cap) {
    size_t n = strcspn(dep, "<>=*?[");
    copy_field(out, cap, dep, n);
}

/* Packages visited in this run (dependency cycles end here). */
static char visited[512][64];
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

static int install(const char *name, int depth) {
    if (depth > 64) {
        return die("dependency chain too deep at ", name);
    }
    if (installed(name) || seen(name)) {
        return 0;
    }
    static char line[8192]; /* reused by the recursive calls: copy out of it */
    if (find_package(name, line, sizeof line) != 0) {
        /* "foo-1.0_1" (an exact pkgver pattern): try its name */
        const char *dash = strrchr(name, '-');
        if (dash != NULL && dash != name) {
            char base[128];
            copy_field(base, sizeof base, name, (size_t)(dash - name));
            return install(base, depth);
        }
        say("get-void: not in the repository: ", name, NULL);
        return depth == 0 ? 1 : 0; /* a missing dependency is reported, not fatal */
    }
    char pkgver[128], sha[72];
    char *sp = strchr(line, ' ');
    if (sp == NULL || strlen(sp + 1) < 64) {
        return die("bad index line for ", name);
    }
    copy_field(pkgver, sizeof pkgver, line, (size_t)(sp - line));
    copy_field(sha, sizeof sha, sp + 1, 64);

    /* Dependencies first. */
    char *deps = strdup(sp + 1 + 64);
    if (deps == NULL) {
        return die("out of memory", NULL);
    }
    for (char *d = deps; *d == ' ';) {
        char dep[128], dn[128];
        size_t n = strcspn(d + 1, " ");
        copy_field(dep, sizeof dep, d + 1, n);
        d += 1 + n;
        dep_name(dep, dn, sizeof dn);
        if (dn[0] != '\0' && install(dn, depth + 1) != 0) {
            free(deps);
            return 1;
        }
    }
    free(deps);

    char url[512], cache[PATH_MAX_GV], file[160], hex[65];
    copy_field(file, sizeof file, pkgver, strlen(pkgver));
    strcat(file, "." VOID_ARCH ".xbps");
    strcpy(url, mirror);
    strcat(url, VOID_REPO "/");
    strcat(url, file);
    if (db_path(cache, file) != 0) {
        return die("root path too long", NULL);
    }
    say("get-void: ", pkgver, NULL);
    if (download(url, cache) != 0) {
        return die("download failed: ", url);
    }
    if (file_sha256(cache, hex) != 0 || strcmp(hex, sha) != 0) {
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
    int rc = unzstd_tar(cache, &t);
    unpack_end(&t);
    unlink(cache);
    if (rc != 0 || u.failed) {
        return die("cannot unpack ", file);
    }
    mark_installed(name, pkgver);
    return 0;
}

/* The directories and symlinks Void's base-files package provides. */
static void layout(void) {
    static const char *dirs[] = {"usr/bin", "usr/lib", "usr/share", "etc", "tmp", "var/db/get-void/pkgs",
                                 "dev", "proc"};
    static const char *links[][2] = {
        {"bin", "usr/bin"}, {"sbin", "usr/bin"}, {"lib", "usr/lib"}, {"usr/sbin", "bin"},
        {"lib64", "usr/lib"}, {"usr/lib64", "lib"}, /* musl's ld.so links via /usr/lib64 */
    };
    char path[PATH_MAX_GV];
    mkdirs(root, 1);
    for (size_t i = 0; i < sizeof dirs / sizeof dirs[0]; i++) {
        if (under_root(path, dirs[i]) == 0) {
            mkdirs(path, 1);
        }
    }
    for (size_t i = 0; i < sizeof links / sizeof links[0]; i++) {
        struct stat st;
        /* myos stat does not follow a final symlink */
        if (under_root(path, links[i][0]) == 0 && stat(path, &st) != 0) {
            symlink(links[i][1], path);
        }
    }
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
        fputs("usage: get-void [-r ROOT] [-u] PACKAGE...\n"
              "  Install Void Linux (" VOID_ARCH ") packages and their dependencies into\n"
              "  ROOT (default /tmp/void); run them with: linux --root ROOT PROGRAM\n"
              "  -u  refresh the repository index\n",
              stderr);
        return 2;
    }
    const char *m = getenv("GET_VOID_MIRROR");
    copy_field(mirror, sizeof mirror, m != NULL ? m : DEFAULT_MIRROR, sizeof mirror);
    if (root[0] != '/' || strlen(root) > 100) {
        return die("ROOT must be an absolute path of at most 100 bytes: ", root);
    }
    layout();
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
    say("get-void: done; run with: linux --root ", root, " PROGRAM");
    return 0;
}
