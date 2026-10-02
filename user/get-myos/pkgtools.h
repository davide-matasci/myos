/* pkgtools: shared by get-myos and get-alpine, see pkgtools.c. */
#ifndef MYOS_PKGTOOLS_H
#define MYOS_PKGTOOLS_H

#include <stddef.h>
#include <stdint.h>

#define PATH_MAX_GV 256

/* The program name messages start with, and the root files go under. */
extern const char *pkg_prog;
extern const char *pkg_root;

/* "<prog>: a b c" on stderr; die() returns 1 for `return die(...)`. */
void say(const char *a, const char *b, const char *c);
int die(const char *a, const char *b);

/* out = pkg_root + "/" + rel (rel without a leading '/'); -1 if too long. */
int under_root(char *out, const char *rel);
/* mkdir -p of path's directories (and path itself if `self`). */
void mkdirs(const char *path, int self);

/* curl -fsSL URL -o DEST, three attempts; -1 when every attempt failed. */
int download(const char *url, const char *dest);

typedef struct {
    uint32_t h[8];
    uint64_t len;
    uint8_t buf[64];
    size_t n;
} sha256;

void sha256_init(sha256 *s);
void sha256_update(sha256 *s, const uint8_t *p, size_t n);
/* Lowercase hex digest into hex[65]. */
void sha256_hex(sha256 *s, char *hex);

/* A streaming tar reader: feed it bytes, it calls back per entry. */
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

void copy_field(char *out, size_t cap, const char *s, size_t n);
void tar_feed(tar *t, const uint8_t *p, size_t n);

/* Inflate the gzip members of the file at `path` one after the other. The
 * compressed bytes of member i go to in(i), the decompressed ones to out(i);
 * either may be NULL. Returns the number of members, or -1. */
int gunzip_members(const char *path, void (*in)(void *ctx, int member, const uint8_t *p, size_t n),
                   void (*out)(void *ctx, int member, const uint8_t *p, size_t n), void *ctx);

#endif
