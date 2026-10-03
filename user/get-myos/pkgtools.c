/*
 * pkgtools: what get-myos (user/get-myos) and get-alpine (linux-compat)
 * share to fetch and unpack packages: messages, paths under a root,
 * downloading with curl, SHA-256, a streaming tar reader (ustar with pax
 * and GNU long names) and gzip inflation with the zlib port.
 *
 * Uses fputs, not printf: newlib's printf needs extra soft-float helpers
 * on aarch64 and riscv64.
 */
#include "pkgtools.h"

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#include "zlib.h"

const char *pkg_prog = "pkgtools";
const char *pkg_root = "/tmp";

/* ---- messages ----------------------------------------------------------- */

void say(const char *a, const char *b, const char *c) {
    fputs(pkg_prog, stderr);
    fputs(": ", stderr);
    fputs(a, stderr);
    if (b != NULL) {
        fputs(b, stderr);
    }
    if (c != NULL) {
        fputs(c, stderr);
    }
    fputs("\n", stderr);
}

int die(const char *a, const char *b) {
    say(a, b, NULL);
    return 1;
}

/* ---- paths -------------------------------------------------------------- */

/* out = root + "/" + rel (rel without a leading '/'). */
int under_root(char *out, const char *rel) {
    size_t r = strlen(pkg_root), n = strlen(rel);
    if (r + 1 + n + 1 > PATH_MAX_GV) {
        return -1;
    }
    memcpy(out, pkg_root, r);
    out[r] = '/';
    memcpy(out + r + 1, rel, n + 1);
    return 0;
}

/* mkdir -p of path's directories (and path itself if `self`). */
void mkdirs(const char *path, int self) {
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

/* curl's exit status when the server cannot resume a transfer. */
#define CURL_CANNOT_RESUME 33

/* One curl run, appending to what earlier runs left in dest (-C -): curl's
 * exit status (-1 if it did not run). A transfer is cut off only when it
 * stalls (under 1 KB/s for a minute), not after a fixed time: big packages
 * take long under emulation. */
static int download_once(const char *url, const char *dest) {
    pid_t pid = fork();
    if (pid < 0) {
        return -1;
    }
    if (pid == 0) {
        char *argv[] = {"curl", "-fsSL", "--connect-timeout", "30", "--speed-limit", "1024",
                        "--speed-time", "60", "-C", "-", "-o", (char *)dest, (char *)url, NULL};
        execvp("curl", argv);
        _exit(127);
    }
    int status = 0;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status)) {
        return -1;
    }
    return WEXITSTATUS(status);
}

static long long file_size(const char *path) {
    struct stat st;
    return stat(path, &st) == 0 ? (long long)st.st_size : -1;
}

int download_open(const char *url, int *pid) {
    int fds[2];
    if (pipe(fds) != 0) {
        return -1;
    }
    pid_t child = fork();
    if (child < 0) {
        close(fds[0]);
        close(fds[1]);
        return -1;
    }
    if (child == 0) {
        close(fds[0]);
        dup2(fds[1], 1);
        close(fds[1]);
        /* `-o -`: libgloss reports fds 0-2 as a tty whatever they are, and
         * curl refuses to write binary data to a terminal otherwise. */
        char *argv[] = {"curl", "-fsSL", "--connect-timeout", "30", "--max-time", "900",
                        "-o", "-", (char *)url, NULL};
        execvp("curl", argv);
        _exit(127);
    }
    close(fds[1]);
    *pid = (int)child;
    return fds[0];
}

int download_close(int fd, int pid) {
    close(fd);
    int status = 0;
    if (waitpid((pid_t)pid, &status, 0) != (pid_t)pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        return -1;
    }
    return 0;
}

/* A transient connect failure should not fail the whole install. */
int download(const char *url, const char *dest) {
    /* Mirrors drop long transfers: each retry resumes where the last one
     * stopped (or starts over from a server that cannot resume), and three
     * in a row that get no further than before give up. */
    unlink(dest);
    long long best = 0;
    for (int stuck = 0;;) {
        int rc = download_once(url, dest);
        if (rc == 0) {
            return 0;
        }
        long long now = file_size(dest);
        stuck = now > best ? 0 : stuck + 1;
        if (now > best) {
            best = now;
        }
        if (rc == CURL_CANNOT_RESUME) {
            unlink(dest);
        }
        if (stuck == 3) {
            unlink(dest);
            return -1;
        }
        say("retrying ", url, NULL);
        sleep(2);
    }
}

/* ---- sha256 ------------------------------------------------------------- */

static const uint32_t K256[64] = {
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
        uint32_t t1 = h + (ROR(e, 6) ^ ROR(e, 11) ^ ROR(e, 25)) + ((e & f) ^ (~e & g)) + K256[i] + w[i];
        uint32_t t2 = (ROR(a, 2) ^ ROR(a, 13) ^ ROR(a, 22)) + ((a & b) ^ (a & c) ^ (b & c));
        h = g, g = f, f = e, e = d + t1, d = c, c = b, b = a, a = t1 + t2;
    }
    s->h[0] += a, s->h[1] += b, s->h[2] += c, s->h[3] += d;
    s->h[4] += e, s->h[5] += f, s->h[6] += g, s->h[7] += h;
}

void sha256_init(sha256 *s) {
    static const uint32_t iv[8] = {0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                                   0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
    memcpy(s->h, iv, sizeof iv);
    s->len = 0;
    s->n = 0;
}

void sha256_update(sha256 *s, const uint8_t *p, size_t n) {
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
void sha256_hex(sha256 *s, char *hex) {
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

static uint64_t octal(const uint8_t *p, size_t n) {
    uint64_t v = 0;
    for (size_t i = 0; i < n && p[i] >= '0' && p[i] <= '7'; i++) {
        v = v * 8 + (uint64_t)(p[i] - '0');
    }
    return v;
}

void copy_field(char *out, size_t cap, const char *s, size_t n) {
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

void tar_feed(tar *t, const uint8_t *p, size_t n) {
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

/* ---- concatenated gzip streams ------------------------------------------ */

/* Inflate the gzip members of the file at `path` (or of `fd`, read to its
 * end) one after the other. The compressed bytes of member i go to in(i),
 * the decompressed ones to out(i); either may be NULL. Returns the number
 * of members, or -1. */
int gunzip_members(const char *path, void (*in)(void *ctx, int member, const uint8_t *p, size_t n),
                   void (*out)(void *ctx, int member, const uint8_t *p, size_t n), void *ctx) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return -1;
    }
    int members = gunzip_fd(fd, in, out, ctx);
    close(fd);
    return members;
}

int gunzip_fd(int fd, void (*in)(void *ctx, int member, const uint8_t *p, size_t n),
                          void (*out)(void *ctx, int member, const uint8_t *p, size_t n), void *ctx) {
    static uint8_t ibuf[16384], obuf[32768];
    z_stream zs;
    memset(&zs, 0, sizeof zs);
    if (inflateInit2(&zs, 16 + MAX_WBITS) != Z_OK) {
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
    return n == 0 && !open_member ? member : -1;
}
