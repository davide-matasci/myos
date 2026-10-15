/* The OpenSSL API over mbedtls (myos_openssl.h): memory, the version, the
 * random generator, the error queue, stacks, BIOs, keys and digests. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <sys/time.h>
#include <unistd.h>

#include <mbedtls/entropy.h>
#include <mbedtls/error.h>
#include <mbedtls/platform_time.h>

#include "myos_ssl_int.h"

/* ---- memory, version ------------------------------------------------ */

void *CRYPTO_malloc(size_t num, const char *file, int line) {
    (void)file;
    (void)line;
    return malloc(num);
}

void CRYPTO_free(void *ptr, const char *file, int line) {
    (void)file;
    (void)line;
    free(ptr);
}

const char *OpenSSL_version(int type) {
    (void)type;
    return OPENSSL_VERSION_TEXT;
}

/* ---- randomness ----------------------------------------------------- */

/* mbedtls's entropy source: the kernel's (getrandom reads /dev/urandom). */
int mbedtls_hardware_poll(void *data, unsigned char *output, size_t len, size_t *olen) {
    (void)data;
    ssize_t n = getrandom(output, len, 0);
    if (n < 0) {
        return MBEDTLS_ERR_ENTROPY_SOURCE_FAILED;
    }
    *olen = (size_t)n;
    return 0;
}

mbedtls_ms_time_t mbedtls_ms_time(void) {
    struct timeval tv;
    if (gettimeofday(&tv, NULL) != 0) {
        return 0;
    }
    return (mbedtls_ms_time_t)tv.tv_sec * 1000 + (mbedtls_ms_time_t)(tv.tv_usec / 1000);
}

/* One generator for the process, seeded at its first use. A worker forked
 * from a master that used it reseeds, so that the two never share one
 * stream (the process id it was seeded in tells). */
mbedtls_ctr_drbg_context *myos_drbg(void) {
    static mbedtls_entropy_context entropy;
    static mbedtls_ctr_drbg_context drbg;
    static pid_t seeded;
    pid_t pid = getpid();
    if (seeded == 0) {
        mbedtls_entropy_init(&entropy);
        mbedtls_ctr_drbg_init(&drbg);
        if (mbedtls_ctr_drbg_seed(&drbg, mbedtls_entropy_func, &entropy,
                (const unsigned char *)"nginx", 5) != 0) {
            return NULL;
        }
        seeded = pid;
    } else if (seeded != pid) {
        if (mbedtls_ctr_drbg_reseed(&drbg, NULL, 0) != 0) {
            return NULL;
        }
        seeded = pid;
    }
    return &drbg;
}

int myos_rng(void *p, unsigned char *out, size_t len) {
    mbedtls_ctr_drbg_context *drbg = myos_drbg();
    (void)p;
    return drbg != NULL ? mbedtls_ctr_drbg_random(drbg, out, len)
                        : MBEDTLS_ERR_CTR_DRBG_ENTROPY_SOURCE_FAILED;
}

int RAND_bytes(unsigned char *buf, int num) {
    return num >= 0 && myos_rng(NULL, buf, (size_t)num) == 0;
}

/* ---- errors --------------------------------------------------------- */

/* The queue OpenSSL keeps per thread: nginx's workers have one thread. */
#define ERR_QUEUE 16
#define ERR_DATA 120
static struct {
    unsigned long code;
    char data[ERR_DATA];
} err_q[ERR_QUEUE];
static int err_first, err_count;

void myos_err_put(int lib, int reason, const char *data) {
    int i;
    if (err_count == ERR_QUEUE) {
        err_first = (err_first + 1) % ERR_QUEUE;
        err_count--;
    }
    i = (err_first + err_count++) % ERR_QUEUE;
    err_q[i].code = ERR_PACK(lib, 0, reason);
    err_q[i].data[0] = '\0';
    if (data != NULL) {
        strncpy(err_q[i].data, data, ERR_DATA - 1);
        err_q[i].data[ERR_DATA - 1] = '\0';
    }
}

/* An mbedtls failure: the reason, with mbedtls's words for it. */
void myos_err_mbedtls(int lib, int reason, int ret) {
    char text[ERR_DATA];
    mbedtls_strerror(ret, text, sizeof text);
    myos_err_put(lib, reason, text);
}

unsigned long ERR_get_error(void) {
    unsigned long e;
    if (err_count == 0) {
        return 0;
    }
    e = err_q[err_first].code;
    err_first = (err_first + 1) % ERR_QUEUE;
    err_count--;
    return e;
}

unsigned long ERR_peek_error(void) {
    return err_count ? err_q[err_first].code : 0;
}

unsigned long ERR_peek_last_error(void) {
    return err_count ? err_q[(err_first + err_count - 1) % ERR_QUEUE].code : 0;
}

unsigned long ERR_peek_error_line_data(const char **file, int *line,
    const char **data, int *flags) {
    if (file != NULL) {
        *file = "";
    }
    if (line != NULL) {
        *line = 0;
    }
    if (err_count == 0) {
        if (data != NULL) {
            *data = "";
        }
        if (flags != NULL) {
            *flags = 0;
        }
        return 0;
    }
    if (data != NULL) {
        *data = err_q[err_first].data;
    }
    if (flags != NULL) {
        *flags = err_q[err_first].data[0] ? ERR_TXT_STRING : 0;
    }
    return err_q[err_first].code;
}

void ERR_clear_error(void) {
    err_count = 0;
}

static const char *err_lib_name(int lib) {
    switch (lib) {
    case ERR_LIB_SYS: return "system library";
    case ERR_LIB_PEM: return "PEM routines";
    case ERR_LIB_X509: return "x509 certificate routines";
    case ERR_LIB_EVP: return "digital envelope routines";
    case ERR_LIB_SSL: return "SSL routines";
    default: return "lib";
    }
}

static const char *err_reason_name(int lib, int reason) {
    if (lib == ERR_LIB_PEM && reason == PEM_R_NO_START_LINE) {
        return "no start line";
    }
    if (lib == ERR_LIB_X509 && reason == X509_R_KEY_VALUES_MISMATCH) {
        return "key values mismatch";
    }
    if (lib != ERR_LIB_SSL) {
        return NULL;
    }
    switch (reason) {
    case SSL_R_HTTP_REQUEST: return "http request";
    case SSL_R_HTTPS_PROXY_REQUEST: return "https proxy request";
    case SSL_R_NO_SHARED_CIPHER: return "no shared cipher";
    case SSL_R_UNSUPPORTED_PROTOCOL: return "unsupported protocol";
    case SSL_R_WRONG_VERSION_NUMBER: return "wrong version number";
    case SSL_R_UNEXPECTED_MESSAGE: return "unexpected message";
    case SSL_R_DECRYPTION_FAILED_OR_BAD_RECORD_MAC: return "decryption failed or bad record mac";
    case SSL_R_CERTIFICATE_VERIFY_FAILED: return "certificate verify failed";
    case SSL_R_NO_CERTIFICATE_SET: return "no certificate set";
    case SSL_R_RECORD_LENGTH_MISMATCH: return "record length mismatch";
    default: return NULL;
    }
}

/* "error:<code>:<library>::<reason>", OpenSSL 1.1.1's form. */
void ERR_error_string_n(unsigned long e, char *buf, size_t len) {
    int lib = ERR_GET_LIB(e), reason = ERR_GET_REASON(e);
    const char *r = err_reason_name(lib, reason);
    if (len == 0) {
        return;
    }
    if (r != NULL) {
        snprintf(buf, len, "error:%08lX:%s::%s", e, err_lib_name(lib), r);
    } else if (lib == ERR_LIB_SSL && reason >= SSL_AD_REASON_OFFSET) {
        snprintf(buf, len, "error:%08lX:%s::alert %d", e, err_lib_name(lib),
            reason - SSL_AD_REASON_OFFSET);
    } else {
        snprintf(buf, len, "error:%08lX:%s::reason(%d)", e, err_lib_name(lib), reason);
    }
}

/* ---- stacks --------------------------------------------------------- */

OPENSSL_STACK *OPENSSL_sk_new(int (*cmp)(const void *, const void *)) {
    OPENSSL_STACK *st = calloc(1, sizeof *st);
    if (st != NULL) {
        st->cmp = cmp;
    }
    return st;
}

OPENSSL_STACK *OPENSSL_sk_new_null(void) {
    return OPENSSL_sk_new(NULL);
}

OPENSSL_STACK *OPENSSL_sk_dup(const OPENSSL_STACK *st) {
    OPENSSL_STACK *d;
    if (st == NULL) {
        return NULL;
    }
    d = OPENSSL_sk_new(st->cmp);
    if (d == NULL) {
        return NULL;
    }
    for (int i = 0; i < st->num; i++) {
        if (!OPENSSL_sk_push(d, st->data[i])) {
            OPENSSL_sk_free(d);
            return NULL;
        }
    }
    return d;
}

int OPENSSL_sk_num(const OPENSSL_STACK *st) {
    return st != NULL ? st->num : -1;
}

void *OPENSSL_sk_value(const OPENSSL_STACK *st, int i) {
    return st != NULL && i >= 0 && i < st->num ? st->data[i] : NULL;
}

int OPENSSL_sk_push(OPENSSL_STACK *st, const void *data) {
    if (st == NULL) {
        return 0;
    }
    if (st->num == st->cap) {
        int cap = st->cap ? st->cap * 2 : 4;
        void **n = realloc(st->data, (size_t)cap * sizeof *n);
        if (n == NULL) {
            return 0;
        }
        st->data = n;
        st->cap = cap;
    }
    st->data[st->num++] = (void *)data;
    return st->num;
}

void *OPENSSL_sk_shift(OPENSSL_STACK *st) {
    void *v;
    if (st == NULL || st->num == 0) {
        return NULL;
    }
    v = st->data[0];
    memmove(st->data, st->data + 1, (size_t)(--st->num) * sizeof *st->data);
    return v;
}

void *OPENSSL_sk_pop(OPENSSL_STACK *st) {
    return st != NULL && st->num > 0 ? st->data[--st->num] : NULL;
}

/* With a comparison: the index of an equal element (X509_NAME_cmp for the
 * client CA names), else of the same pointer. */
int OPENSSL_sk_find(OPENSSL_STACK *st, const void *data) {
    if (st == NULL) {
        return -1;
    }
    for (int i = 0; i < st->num; i++) {
        if (st->cmp != NULL ? st->cmp(&st->data[i], &data) == 0 : st->data[i] == data) {
            return i;
        }
    }
    return -1;
}

void OPENSSL_sk_free(OPENSSL_STACK *st) {
    if (st != NULL) {
        free(st->data);
        free(st);
    }
}

void OPENSSL_sk_pop_free(OPENSSL_STACK *st, void (*func)(void *)) {
    if (st == NULL) {
        return;
    }
    for (int i = 0; i < st->num; i++) {
        func(st->data[i]);
    }
    OPENSSL_sk_free(st);
}

/* ---- ex_data -------------------------------------------------------- */

/* The next free slot of an object kind (0 SSL, 1 SSL_CTX, 2 X509). */
int myos_ex_index(int kind) {
    static int next[3];
    return next[kind] < MYOS_EX_SLOTS ? next[kind]++ : -1;
}

/* ---- BIO ------------------------------------------------------------ */

static const struct bio_method_st { int unused; } bio_mem_method;

const BIO_METHOD *BIO_s_mem(void) {
    return &bio_mem_method;
}

BIO *myos_bio_mem(void) {
    BIO *b = calloc(1, sizeof *b);
    if (b == NULL) {
        return NULL;
    }
    b->fd = -1;
    b->buf = calloc(1, 1);
    if (b->buf == NULL) {
        free(b);
        return NULL;
    }
    b->cap = 1;
    return b;
}

BIO *BIO_new(const BIO_METHOD *type) {
    (void)type;
    return myos_bio_mem();
}

int myos_bio_append(BIO *b, const void *data, size_t len) {
    if (b->len + len + 1 > b->cap) {
        size_t cap = b->cap * 2 > b->len + len + 1 ? b->cap * 2 : b->len + len + 1;
        unsigned char *n = realloc(b->buf, cap);
        if (n == NULL) {
            return 0;
        }
        b->buf = n;
        b->cap = cap;
    }
    memcpy(b->buf + b->len, data, len);
    b->len += len;
    b->buf[b->len] = '\0';
    return 1;
}

int myos_bio_puts(BIO *b, const char *s) {
    return myos_bio_append(b, s, strlen(s));
}

BIO *BIO_new_mem_buf(const void *buf, int len) {
    BIO *b = myos_bio_mem();
    if (b != NULL && !myos_bio_append(b, buf, len < 0 ? strlen(buf) : (size_t)len)) {
        BIO_free(b);
        return NULL;
    }
    return b;
}

/* A file is read whole: nginx reads certificates, keys and CRLs. */
BIO *BIO_new_file(const char *filename, const char *mode) {
    unsigned char chunk[4096];
    ssize_t n;
    BIO *b;
    int fd;
    if (mode[0] != 'r') {
        errno = EINVAL;
        myos_err_put(ERR_LIB_SYS, EINVAL, filename);
        return NULL;
    }
    fd = open(filename, O_RDONLY);
    if (fd < 0) {
        myos_err_put(ERR_LIB_SYS, errno, filename);
        return NULL;
    }
    b = myos_bio_mem();
    while (b != NULL && (n = read(fd, chunk, sizeof chunk)) > 0) {
        if (!myos_bio_append(b, chunk, (size_t)n)) {
            BIO_free(b);
            b = NULL;
        }
    }
    close(fd);
    return b;
}

int BIO_free(BIO *a) {
    if (a == NULL) {
        return 0;
    }
    free(a->buf);
    free(a);
    return 1;
}

int BIO_read(BIO *b, void *data, int dlen) {
    size_t n = b->len - b->pos;
    if (dlen <= 0 || n == 0) {
        return 0;
    }
    if (n > (size_t)dlen) {
        n = (size_t)dlen;
    }
    memcpy(data, b->buf + b->pos, n);
    b->pos += n;
    return (int)n;
}

int BIO_write(BIO *b, const void *data, int dlen) {
    return dlen >= 0 && myos_bio_append(b, data, (size_t)dlen) ? dlen : -1;
}

long BIO_ctrl(BIO *bp, int cmd, long larg, void *parg) {
    (void)larg;
    switch (cmd) {
    case BIO_CTRL_RESET:
        bp->pos = 0;
        return 1;
    case BIO_CTRL_PENDING:
        return (long)(bp->len - bp->pos);
    case BIO_CTRL_INFO:
        if (parg != NULL) {
            *(char **)parg = (char *)bp->buf + bp->pos;
        }
        return (long)(bp->len - bp->pos);
    default:
        return 0;
    }
}

/* ---- EVP: keys and digests ------------------------------------------ */

EVP_PKEY *EVP_PKEY_new(void) {
    EVP_PKEY *k = calloc(1, sizeof *k);
    if (k != NULL) {
        k->refs = 1;
        mbedtls_pk_init(&k->pk);
    }
    return k;
}

void EVP_PKEY_free(EVP_PKEY *pkey) {
    if (pkey != NULL && --pkey->refs == 0) {
        mbedtls_pk_free(&pkey->pk);
        free(pkey);
    }
}

int EVP_PKEY_up_ref(EVP_PKEY *pkey) {
    pkey->refs++;
    return 1;
}

static const EVP_MD md_sha1 = {MBEDTLS_MD_SHA1};
static const EVP_MD md_sha256 = {MBEDTLS_MD_SHA256};

const EVP_MD *EVP_sha1(void) {
    return &md_sha1;
}

const EVP_MD *EVP_sha256(void) {
    return &md_sha256;
}

EVP_MD_CTX *EVP_MD_CTX_new(void) {
    EVP_MD_CTX *c = calloc(1, sizeof *c);
    if (c != NULL) {
        mbedtls_md_init(&c->md);
    }
    return c;
}

void EVP_MD_CTX_free(EVP_MD_CTX *ctx) {
    if (ctx != NULL) {
        mbedtls_md_free(&ctx->md);
        free(ctx);
    }
}

int EVP_DigestInit_ex(EVP_MD_CTX *ctx, const EVP_MD *type, void *impl) {
    (void)impl;
    mbedtls_md_free(&ctx->md);
    mbedtls_md_init(&ctx->md);
    return mbedtls_md_setup(&ctx->md, mbedtls_md_info_from_type(type->type), 0) == 0
        && mbedtls_md_starts(&ctx->md) == 0;
}

int EVP_DigestUpdate(EVP_MD_CTX *ctx, const void *d, size_t cnt) {
    return mbedtls_md_update(&ctx->md, d, cnt) == 0;
}

int EVP_DigestFinal_ex(EVP_MD_CTX *ctx, unsigned char *md, unsigned int *s) {
    if (mbedtls_md_finish(&ctx->md, md) != 0) {
        return 0;
    }
    if (s != NULL) {
        *s = mbedtls_md_get_size(mbedtls_md_info_from_ctx(&ctx->md));
    }
    return 1;
}
