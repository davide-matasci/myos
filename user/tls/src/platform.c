/* myos TLS platform glue: entropy, time, and a thin client API over an fd. */
#include <stddef.h>
#include <stdint.h>

#include "mbedtls/platform.h"
#include "mbedtls/ctr_drbg.h"
#include "mbedtls/entropy.h"
#include "mbedtls/error.h"
#include "mbedtls/memory_buffer_alloc.h"
#include "mbedtls/ssl.h"
#include "mbedtls/x509_crt.h"

/* From ca_bundle.o linked into libmbedx509.a */
extern const char myos_ca_bundle_pem[];
extern const unsigned myos_ca_bundle_pem_len;

/* Rust syscall wrappers */
extern int myos_tls_gettimeofday_sec(int64_t *sec);
extern int myos_tls_fd_read(int fd, unsigned char *buf, size_t len);
extern int myos_tls_fd_write(int fd, const unsigned char *buf, size_t len);

/* Full Mozilla CA parse + TLS 1.2 buffers need well over 256 KiB. */
static unsigned char g_heap[2 * 1024 * 1024];
static int g_mem_ready;

mbedtls_time_t myos_mbedtls_time(mbedtls_time_t *t) {
    int64_t sec = 0;
    if (myos_tls_gettimeofday_sec(&sec) != 0) {
        sec = 0;
    }
    if (t) {
        *t = (mbedtls_time_t)sec;
    }
    return (mbedtls_time_t)sec;
}

mbedtls_ms_time_t mbedtls_ms_time(void) {
    int64_t sec = 0;
    (void)myos_tls_gettimeofday_sec(&sec);
    return (mbedtls_ms_time_t)sec * 1000;
}


int mbedtls_hardware_poll(void *data, unsigned char *output, size_t len, size_t *olen) {
    (void)data;
    int64_t sec = 0;
    (void)myos_tls_gettimeofday_sec(&sec);
    /* Mix wall clock + ASLR-ish addresses into a simple xorshift stream. */
    uint64_t s = (uint64_t)sec;
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

static void ensure_mem(void) {
    if (!g_mem_ready) {
        /* init also installs mbedtls_calloc/free to the buffer allocator */
        mbedtls_memory_buffer_alloc_init(g_heap, sizeof(g_heap));
        g_mem_ready = 1;
    }
}

typedef struct {
    mbedtls_ssl_context ssl;
    mbedtls_ssl_config conf;
    mbedtls_entropy_context entropy;
    mbedtls_ctr_drbg_context ctr_drbg;
    mbedtls_x509_crt cacert;
    int fd;
    int ready;
} myos_tls_conn;

/* /net TCP reads return 0 when the rx queue is empty (non-blocking style).
 * Returning WANT_READ/WRITE immediately makes mbedtls busy-loop in userspace
 * and starves netd on a single core. Poll with real read/write syscalls
 * (same approach as user/dns DATA_POLLS) so the scheduler can run netd.
 */
/* Budget for empty /net TCP I/O. Writes usually complete in 1 syscall
 * (netfs enqueues REQ_SEND). Recv must wait for netd to TX ClientHello and
 * pump ServerHello/certs into DATA — cert chains need headroom under load.
 */
enum { BIO_POLLS = 800000 };

static int bio_send(void *ctx, const unsigned char *buf, size_t len) {
    myos_tls_conn *c = (myos_tls_conn *)ctx;
    for (int i = 0; i < BIO_POLLS; i++) {
        int n = myos_tls_fd_write(c->fd, buf, len);
        if (n < 0) {
            return MBEDTLS_ERR_SSL_INTERNAL_ERROR;
        }
        if (n > 0) {
            return n;
        }
    }
    /* Distinct from recv timeout so CI/logs show which side stuck. */
    return -0x004E; /* MBEDTLS_ERR_NET_SEND_FAILED: bio_send poll exhausted */
}

static int bio_recv(void *ctx, unsigned char *buf, size_t len) {
    myos_tls_conn *c = (myos_tls_conn *)ctx;
    for (int i = 0; i < BIO_POLLS; i++) {
        int n = myos_tls_fd_read(c->fd, buf, len);
        if (n < 0) {
            return MBEDTLS_ERR_SSL_INTERNAL_ERROR;
        }
        if (n > 0) {
            return n;
        }
    }
    return MBEDTLS_ERR_SSL_TIMEOUT;
}

int myos_tls_handshake(myos_tls_conn *c, int fd, const char *sni_host) {
    int ret;
    const char *pers = "myos-tls";

    ensure_mem();
    mbedtls_platform_set_time(myos_mbedtls_time);
    {
        mbedtls_time_t now = myos_mbedtls_time(NULL);
        /* Certs for public sites are not valid in 1970; refuse early with a
         * distinctive code so CI can tell RTC mapping/time apart from TLS. */
        if (now < (mbedtls_time_t)1700000000) { /* ~2023-11-14 */
            return MBEDTLS_ERR_SSL_BAD_INPUT_DATA;
        }
    }

    mbedtls_ssl_init(&c->ssl);
    mbedtls_ssl_config_init(&c->conf);
    mbedtls_entropy_init(&c->entropy);
    mbedtls_ctr_drbg_init(&c->ctr_drbg);
    mbedtls_x509_crt_init(&c->cacert);
    c->fd = fd;
    c->ready = 0;

    ret = mbedtls_ctr_drbg_seed(&c->ctr_drbg, mbedtls_entropy_func, &c->entropy,
                                (const unsigned char *)pers, 8);
    if (ret != 0) {
        return ret;
    }

    ret = mbedtls_x509_crt_parse(&c->cacert, (const unsigned char *)myos_ca_bundle_pem,
                                 myos_ca_bundle_pem_len + 1);
    if (ret < 0) {
        return ret;
    }

    ret = mbedtls_ssl_config_defaults(&c->conf, MBEDTLS_SSL_IS_CLIENT,
                                      MBEDTLS_SSL_TRANSPORT_STREAM,
                                      MBEDTLS_SSL_PRESET_DEFAULT);
    if (ret != 0) {
        return ret;
    }
    mbedtls_ssl_conf_min_tls_version(&c->conf, MBEDTLS_SSL_VERSION_TLS1_2);
    mbedtls_ssl_conf_max_tls_version(&c->conf, MBEDTLS_SSL_VERSION_TLS1_2);
    mbedtls_ssl_conf_authmode(&c->conf, MBEDTLS_SSL_VERIFY_REQUIRED);
    mbedtls_ssl_conf_ca_chain(&c->conf, &c->cacert, NULL);
    mbedtls_ssl_conf_rng(&c->conf, mbedtls_ctr_drbg_random, &c->ctr_drbg);

    ret = mbedtls_ssl_setup(&c->ssl, &c->conf);
    if (ret != 0) {
        return ret;
    }
    if (sni_host && sni_host[0]) {
        ret = mbedtls_ssl_set_hostname(&c->ssl, sni_host);
        if (ret != 0) {
            return ret;
        }
    }
    mbedtls_ssl_set_bio(&c->ssl, c, bio_send, bio_recv, NULL);

    while ((ret = mbedtls_ssl_handshake(&c->ssl)) != 0) {
        if (ret != MBEDTLS_ERR_SSL_WANT_READ && ret != MBEDTLS_ERR_SSL_WANT_WRITE) {
            return ret;
        }
    }
    if (mbedtls_ssl_get_verify_result(&c->ssl) != 0) {
        return MBEDTLS_ERR_X509_CERT_VERIFY_FAILED;
    }
    c->ready = 1;
    return 0;
}

int myos_tls_write(myos_tls_conn *c, const unsigned char *buf, size_t len) {
    if (!c || !c->ready) {
        return -1;
    }
    int ret;
    while ((ret = mbedtls_ssl_write(&c->ssl, buf, len)) <= 0) {
        if (ret != MBEDTLS_ERR_SSL_WANT_READ && ret != MBEDTLS_ERR_SSL_WANT_WRITE) {
            return ret;
        }
    }
    return ret;
}

int myos_tls_read(myos_tls_conn *c, unsigned char *buf, size_t len) {
    if (!c || !c->ready) {
        return -1;
    }
    int ret = mbedtls_ssl_read(&c->ssl, buf, len);
    if (ret == MBEDTLS_ERR_SSL_WANT_READ || ret == MBEDTLS_ERR_SSL_WANT_WRITE) {
        return 0;
    }
    if (ret == MBEDTLS_ERR_SSL_PEER_CLOSE_NOTIFY) {
        return 0;
    }
    return ret;
}

void myos_tls_close(myos_tls_conn *c) {
    if (!c) {
        return;
    }
    if (c->ready) {
        mbedtls_ssl_close_notify(&c->ssl);
    }
    mbedtls_x509_crt_free(&c->cacert);
    mbedtls_ssl_free(&c->ssl);
    mbedtls_ssl_config_free(&c->conf);
    mbedtls_ctr_drbg_free(&c->ctr_drbg);
    mbedtls_entropy_free(&c->entropy);
    c->ready = 0;
}

size_t myos_tls_conn_size(void) {
    return sizeof(myos_tls_conn);
}
