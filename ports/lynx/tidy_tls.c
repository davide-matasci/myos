/*
 * tidy_tls.c — mbedtls backend for lynx's OpenSSL-compat tidy_tls API.
 *
 * Mirrors upstream src/tidy_tls.c (GnuTLS) and user/tls/src/platform.c
 * (myos mbedtls client over an fd). Uses ports/mbedtls + /lib/cacert.pem.
 */
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

#include "mbedtls/ctr_drbg.h"
#include "mbedtls/entropy.h"
#include "mbedtls/error.h"
#include "mbedtls/ssl.h"
#include "mbedtls/x509_crt.h"

#include "tidy_tls.h"

#define typeCalloc(type) (type *)calloc(1, sizeof(type))

#ifndef SSL_CERT_FILE_DEFAULT
#define SSL_CERT_FILE_DEFAULT "/lib/cacert.pem"
#endif

/* /net empty-read polling — same rationale as user/tls platform.c */
enum { BIO_POLLS = 800000 };

struct _SSL {
    unsigned long options;
    SSL_CTX *ctx;
    SSL_CIPHER ciphersuite;
    int last_error;
    int state;
    int fd;
    int ready;
    char *hostname;
    int (*verify_callback)(int, X509_STORE_CTX *);
    int verify_mode;

    mbedtls_ssl_context ssl;
    mbedtls_ssl_config conf;
    mbedtls_entropy_context entropy;
    mbedtls_ctr_drbg_context ctr_drbg;
    mbedtls_x509_crt cacert;
    X509 peer_cert; /* shallow view of peer DER if available */
    int peer_valid;
};

static int last_error;
static int library_ready;
static SSL_METHOD client_method;

/* Entropy + time for mbedtls (same as ports/curl/myos_curl_platform.c). */
int mbedtls_hardware_poll(void *data, unsigned char *output, size_t len, size_t *olen) {
    (void)data;
    struct timeval tv;
    uint64_t s = 0;
    if (gettimeofday(&tv, NULL) == 0) {
        s = ((uint64_t)tv.tv_sec << 32) ^ (uint64_t)tv.tv_usec;
    }
    s ^= (uint64_t)(uintptr_t)output << 7;
    s ^= (uint64_t)(uintptr_t)&s << 13;
    s ^= 0xA5A5F00DDEADBEEFULL;
    for (size_t i = 0; i < len; i++) {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s += 0x9E3779B97F4A7C15ULL;
        output[i] = (unsigned char)(s >> 32);
    }
    if (olen) {
        *olen = len;
    }
    return 0;
}

mbedtls_ms_time_t mbedtls_ms_time(void) {
    struct timeval tv;
    if (gettimeofday(&tv, NULL) != 0) {
        return 0;
    }
    return (mbedtls_ms_time_t)tv.tv_sec * 1000 + (mbedtls_ms_time_t)(tv.tv_usec / 1000);
}

static int bio_send(void *ctx, const unsigned char *buf, size_t len) {
    SSL *ssl = (SSL *)ctx;
    for (int i = 0; i < BIO_POLLS; i++) {
        ssize_t n = write(ssl->fd, buf, len);
        if (n < 0) {
            if (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR) {
                continue;
            }
            return MBEDTLS_ERR_SSL_INTERNAL_ERROR;
        }
        if (n > 0) {
            return (int)n;
        }
    }
    return MBEDTLS_ERR_SSL_TIMEOUT;
}

static int bio_recv(void *ctx, unsigned char *buf, size_t len) {
    SSL *ssl = (SSL *)ctx;
    for (int i = 0; i < BIO_POLLS; i++) {
        ssize_t n = read(ssl->fd, buf, len);
        if (n < 0) {
            if (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR) {
                continue;
            }
            return MBEDTLS_ERR_SSL_INTERNAL_ERROR;
        }
        if (n > 0) {
            return (int)n;
        }
        /* n==0: empty /net queue or EOF — keep polling for handshake */
    }
    return MBEDTLS_ERR_SSL_TIMEOUT;
}

const char *ERR_error_string(unsigned long e, char *buffer) {
    static char local[128];
    char *out = buffer ? buffer : local;
    mbedtls_strerror(-(int)e, out, buffer ? 120 : sizeof(local));
    return out;
}

unsigned long ERR_get_error(void) {
    unsigned long rc = (unsigned long)(-last_error);
    last_error = 0;
    return rc;
}

int RAND_bytes(unsigned char *buffer, int num) {
    /* Best-effort: hardware_poll is our CSPRNG seed path. */
    size_t olen = 0;
    return mbedtls_hardware_poll(NULL, buffer, (size_t)num, &olen) == 0 ? 1 : 0;
}

const char *RAND_file_name(char *buffer, size_t len) {
    (void)buffer;
    (void)len;
    return "";
}

int RAND_load_file(const char *name, long maxbytes) {
    (void)name;
    return (int)maxbytes;
}

void RAND_seed(const void *buffer, int num) {
    (void)buffer;
    (void)num;
}

int RAND_status(void) {
    return 1;
}

int RAND_write_file(const char *name) {
    (void)name;
    return 0;
}

int SSL_CIPHER_get_bits(SSL_CIPHER *cipher, int *bits) {
    int result = cipher ? cipher->bits : 0;
    if (bits) {
        *bits = result;
    }
    return result;
}

const char *SSL_CIPHER_get_name(SSL_CIPHER *cipher) {
    return cipher && cipher->name ? cipher->name : "NONE";
}

const char *SSL_CIPHER_get_version(SSL_CIPHER *cipher) {
    return cipher && cipher->version ? cipher->version : "NONE";
}

void SSL_CTX_free(SSL_CTX *ctx) {
    if (!ctx) {
        return;
    }
    free(ctx->method);
    free(ctx);
}

SSL_CTX *SSL_CTX_new(SSL_METHOD *method) {
    SSL_CTX *ctx = typeCalloc(SSL_CTX);
    if (ctx) {
        ctx->method = method;
        ctx->certfile = (char *)SSL_CERT_FILE_DEFAULT;
        ctx->certfile_type = GNUTLS_X509_FMT_PEM;
    }
    return ctx;
}

int SSL_CTX_set_default_verify_paths(SSL_CTX *ctx) {
    (void)ctx;
    return 1;
}

unsigned long SSL_CTX_set_options(SSL_CTX *ctx, unsigned long options) {
    ctx->options |= options;
    return ctx->options;
}

unsigned long SSL_set_options(SSL *ssl, unsigned long options) {
    ssl->options |= options;
    return ssl->options;
}

void SSL_CTX_set_verify(SSL_CTX *ctx, int verify_mode,
                        int (*verify_callback)(int, X509_STORE_CTX *)) {
    ctx->verify_mode = verify_mode;
    ctx->verify_callback = verify_callback;
}

SSL_METHOD *SSLv23_client_method(void) {
    return &client_method;
}

SSL_METHOD *TLS_client_method(void) {
    return &client_method;
}

int SSL_library_init(void) {
    library_ready = 1;
    return 1;
}

void SSL_load_error_strings(void) {
}

SSL *SSL_new(SSL_CTX *ctx) {
    SSL *ssl = typeCalloc(SSL);
    if (!ssl) {
        return NULL;
    }
    ssl->ctx = ctx;
    ssl->options = ctx ? ctx->options : 0;
    ssl->verify_mode = ctx ? ctx->verify_mode : SSL_VERIFY_PEER;
    ssl->verify_callback = ctx ? ctx->verify_callback : NULL;
    ssl->fd = -1;
    mbedtls_ssl_init(&ssl->ssl);
    mbedtls_ssl_config_init(&ssl->conf);
    mbedtls_entropy_init(&ssl->entropy);
    mbedtls_ctr_drbg_init(&ssl->ctr_drbg);
    mbedtls_x509_crt_init(&ssl->cacert);
    return ssl;
}

void SSL_free(SSL *ssl) {
    if (!ssl) {
        return;
    }
    mbedtls_ssl_free(&ssl->ssl);
    mbedtls_ssl_config_free(&ssl->conf);
    mbedtls_ctr_drbg_free(&ssl->ctr_drbg);
    mbedtls_entropy_free(&ssl->entropy);
    mbedtls_x509_crt_free(&ssl->cacert);
    free(ssl->hostname);
    free(ssl);
}

int SSL_set_fd(SSL *ssl, int fd) {
    ssl->fd = fd;
    return 1;
}

int SSL_set_tlsext_host_name(SSL *ssl, char *host) {
    free(ssl->hostname);
    ssl->hostname = host ? strdup(host) : NULL;
    return 1;
}

static int load_ca(SSL *ssl) {
    const char *path = NULL;
    if (ssl->ctx && ssl->ctx->certfile && ssl->ctx->certfile[0]) {
        path = ssl->ctx->certfile;
    } else {
        path = SSL_CERT_FILE_DEFAULT;
    }
    int ret = mbedtls_x509_crt_parse_file(&ssl->cacert, path);
    if (ret < 0) {
        last_error = ret;
        ssl->last_error = ret;
        return ret;
    }
    return 0;
}

int SSL_connect(SSL *ssl) {
    int ret;
    const char *pers = "lynx-mbedtls";

    if (ssl->fd < 0) {
        last_error = -1;
        return 0;
    }

    ret = mbedtls_ctr_drbg_seed(&ssl->ctr_drbg, mbedtls_entropy_func, &ssl->entropy,
                                (const unsigned char *)pers, strlen(pers));
    if (ret != 0) {
        goto fail;
    }

    ret = load_ca(ssl);
    if (ret != 0) {
        goto fail;
    }

    ret = mbedtls_ssl_config_defaults(&ssl->conf, MBEDTLS_SSL_IS_CLIENT,
                                      MBEDTLS_SSL_TRANSPORT_STREAM,
                                      MBEDTLS_SSL_PRESET_DEFAULT);
    if (ret != 0) {
        goto fail;
    }

    mbedtls_ssl_conf_min_tls_version(&ssl->conf, MBEDTLS_SSL_VERSION_TLS1_2);
    mbedtls_ssl_conf_max_tls_version(&ssl->conf, MBEDTLS_SSL_VERSION_TLS1_2);
    if (ssl->verify_mode & SSL_VERIFY_PEER) {
        mbedtls_ssl_conf_authmode(&ssl->conf, MBEDTLS_SSL_VERIFY_REQUIRED);
    } else {
        mbedtls_ssl_conf_authmode(&ssl->conf, MBEDTLS_SSL_VERIFY_NONE);
    }
    mbedtls_ssl_conf_ca_chain(&ssl->conf, &ssl->cacert, NULL);
    mbedtls_ssl_conf_rng(&ssl->conf, mbedtls_ctr_drbg_random, &ssl->ctr_drbg);

    ret = mbedtls_ssl_setup(&ssl->ssl, &ssl->conf);
    if (ret != 0) {
        goto fail;
    }
    if (ssl->hostname && ssl->hostname[0]) {
        ret = mbedtls_ssl_set_hostname(&ssl->ssl, ssl->hostname);
        if (ret != 0) {
            goto fail;
        }
    }
    mbedtls_ssl_set_bio(&ssl->ssl, ssl, bio_send, bio_recv, NULL);

    while ((ret = mbedtls_ssl_handshake(&ssl->ssl)) != 0) {
        if (ret != MBEDTLS_ERR_SSL_WANT_READ && ret != MBEDTLS_ERR_SSL_WANT_WRITE) {
            goto fail;
        }
    }

    if ((ssl->verify_mode & SSL_VERIFY_PEER) &&
        mbedtls_ssl_get_verify_result(&ssl->ssl) != 0) {
        ret = MBEDTLS_ERR_X509_CERT_VERIFY_FAILED;
        goto fail;
    }

    {
        const mbedtls_x509_crt *peer = mbedtls_ssl_get_peer_cert(&ssl->ssl);
        if (peer) {
            ssl->peer_cert.data = (unsigned char *)peer->raw.p;
            ssl->peer_cert.len = (unsigned)peer->raw.len;
            ssl->peer_valid = 1;
        }
    }

    ssl->ciphersuite.name = mbedtls_ssl_get_ciphersuite(&ssl->ssl);
    ssl->ciphersuite.version = mbedtls_ssl_get_version(&ssl->ssl);
    ssl->ciphersuite.bits = 128;

    if (ssl->verify_callback) {
        X509_STORE_CTX store;
        memset(&store, 0, sizeof(store));
        store.ssl = ssl;
        store.cert_list = ssl->peer_valid ? &ssl->peer_cert : NULL;
        (void)ssl->verify_callback(1, &store);
    }

    ssl->state = SSL_ST_OK;
    ssl->ready = 1;
    return 1;

fail:
    last_error = ret;
    ssl->last_error = ret;
    return 0;
}

int SSL_read(SSL *ssl, void *buf, int len) {
    int ret = mbedtls_ssl_read(&ssl->ssl, (unsigned char *)buf, (size_t)len);
    if (ret < 0) {
        if (ret == MBEDTLS_ERR_SSL_WANT_READ || ret == MBEDTLS_ERR_SSL_WANT_WRITE) {
            errno = EAGAIN;
            return -1;
        }
        if (ret == MBEDTLS_ERR_SSL_PEER_CLOSE_NOTIFY) {
            return 0;
        }
        last_error = ret;
        ssl->last_error = ret;
        return -1;
    }
    return ret;
}

int SSL_write(SSL *ssl, const void *buf, int len) {
    int ret = mbedtls_ssl_write(&ssl->ssl, (const unsigned char *)buf, (size_t)len);
    if (ret < 0) {
        if (ret == MBEDTLS_ERR_SSL_WANT_READ || ret == MBEDTLS_ERR_SSL_WANT_WRITE) {
            errno = EAGAIN;
            return -1;
        }
        last_error = ret;
        ssl->last_error = ret;
        return -1;
    }
    return ret;
}

SSL_CIPHER *SSL_get_current_cipher(SSL *ssl) {
    return ssl ? &ssl->ciphersuite : NULL;
}

const X509 *SSL_get_peer_certificate(SSL *ssl) {
    if (!ssl || !ssl->peer_valid) {
        return NULL;
    }
    return &ssl->peer_cert;
}

static X509_NAME cached_name;

X509_NAME *X509_get_issuer_name(const X509 *cert) {
    (void)cert;
    /* Without SSL context we cannot parse; return empty cached name. */
    memset(&cached_name, 0, sizeof(cached_name));
    return &cached_name;
}

X509_NAME *X509_get_subject_name(const X509 *cert) {
    (void)cert;
    memset(&cached_name, 0, sizeof(cached_name));
    return &cached_name;
}

char *X509_NAME_oneline(X509_NAME *name, char *buf, int len) {
    static char local[TIDY_TLS_BUFSIZE];
    char *out = buf ? buf : local;
    size_t cap = buf ? (size_t)len : sizeof(local);
    if (!name || cap == 0) {
        return NULL;
    }
    snprintf(out, cap, "/CN=%s", name->common_name);
    return out;
}
