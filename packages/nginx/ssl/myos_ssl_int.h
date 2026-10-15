/* The objects behind myos_openssl.h's types, shared by its implementation
 * (myos_crypto.c, myos_x509.c, myos_ssl.c). */
#ifndef MYOS_SSL_INT_H
#define MYOS_SSL_INT_H

#include "myos_openssl.h"

#include <mbedtls/ctr_drbg.h>
#include <mbedtls/md.h>
#include <mbedtls/pk.h>
#include <mbedtls/ssl.h>
#include <mbedtls/x509_crl.h>
#include <mbedtls/x509_crt.h>

/* ex_data slots per object (nginx uses a handful of each kind). */
#define MYOS_EX_SLOTS 16

struct openssl_stack_st {
    void **data;
    int num;
    int cap;
    int (*cmp)(const void *, const void *);
};

/* A BIO is a memory buffer: a file's contents (read), a buffer given to
 * BIO_new_mem_buf (read), or what is written (BIO_s_mem). The data always
 * has a NUL after it, for mbedtls's PEM parser. A connection's BIO is
 * `fd >= 0`, only ever compared (SSL_get_rbio, SSL_get_wbio). */
struct bio_st {
    unsigned char *buf;
    size_t len;
    size_t cap;
    size_t pos;
    int fd;
};

/* An ASN.1 string, integer or time: the bytes, and a time's fields. */
struct asn1_string_st {
    int type;
    int length;
    unsigned char *data;
    mbedtls_x509_time time;
};

/* A name: its DER, and the attributes in it, in order. */
#define MYOS_NAME_MAX 32
struct myos_name_attr {
    const unsigned char *oid;
    size_t oid_len;
    int tag;
    ASN1_STRING value;
};

struct x509_name_entry_st {
    ASN1_STRING value;
};

struct x509_name_st {
    unsigned char *der;
    size_t der_len;
    int n;
    struct myos_name_attr attr[MYOS_NAME_MAX];
    X509_NAME_ENTRY entry[MYOS_NAME_MAX];
};

/* One certificate (crt.next is NULL), with what nginx asks of it. */
struct x509_st {
    int refs;
    mbedtls_x509_crt crt;
    X509_NAME *subject;
    X509_NAME *issuer;
    ASN1_INTEGER serial;
    ASN1_TIME not_before;
    ASN1_TIME not_after;
    void *ex[MYOS_EX_SLOTS];
};

struct x509_crl_st {
    int refs;
    mbedtls_x509_crl crl;
};

/* The trusted certificates and the CRLs. */
struct x509_store_st {
    mbedtls_x509_crt cas;
    int ncas;
    mbedtls_x509_crl crls;
    int ncrls;
    unsigned long flags;
};

struct evp_pkey_st {
    int refs;
    mbedtls_pk_context pk;
};

struct evp_md_st {
    mbedtls_md_type_t type;
};

struct evp_md_ctx_st {
    mbedtls_md_context_t md;
};

struct ssl_cipher_st {
    int id;
};

struct ssl_comp_st {
    int id;
};

struct ssl_conf_ctx_st {
    SSL_CTX *ctx;
    unsigned int flags;
};

struct ssl_method_st {
    int unused;
};

/* A session: mbedtls's, serialized (mbedtls_ssl_session_save), with what
 * OpenSSL's has besides: its id, when it was made and for how long. */
struct ssl_session_st {
    int refs;
    unsigned char id[32];
    unsigned int id_len;
    long time;
    long timeout;
    unsigned char sid_ctx[SSL_MAX_SID_CTX_LENGTH];
    unsigned int sid_ctx_len;
    unsigned char *data;
    size_t data_len;
};

/* A key and its certificate chain (SSL_CTX_use_certificate, set0_chain,
 * use_PrivateKey), one per key type as in OpenSSL. */
#define MYOS_CERT_SLOTS 4
struct myos_cert {
    X509 *x509;
    STACK_OF(X509) *chain;
    EVP_PKEY *pkey;
    mbedtls_x509_crt crts;
    int built;
};

/* An mbedtls configuration made from an SSL_CTX for one endpoint, shared
 * by the connections made with it. */
struct myos_conf {
    int refs;
    mbedtls_ssl_config conf;
    int ciphersuites[64];
    uint16_t groups[8];
};

struct ssl_ctx_st {
    int refs;
    unsigned long options;
    int min_version;
    int max_version;
    int verify_mode;
    int verify_depth;
    SSL_verify_cb verify_cb;
    void (*info_cb)(const SSL *ssl, int type, int val);
    int (*servername_cb)(SSL *s, int *ad, void *arg);
    struct myos_cert certs[MYOS_CERT_SLOTS];
    int ncerts;
    int cur_cert;
    X509_STORE *store;
    STACK_OF(X509_NAME) *client_ca;
    long timeout;
    long cache_mode;
    unsigned char sid_ctx[SSL_MAX_SID_CTX_LENGTH];
    unsigned int sid_ctx_len;
    int (*new_session_cb)(SSL *, SSL_SESSION *);
    void (*remove_session_cb)(SSL_CTX *, SSL_SESSION *);
    SSL_SESSION *(*get_session_cb)(SSL *, const unsigned char *, int, int *);
    int ciphersuites[64];
    int nciphersuites;
    uint16_t groups[8];
    int ngroups;
    struct myos_conf *conf[2];  /* server, client: made at the first SSL_new */
    void *ex[MYOS_EX_SLOTS];
};

/* A connection: mbedtls's context first, so that its callbacks find it. */
struct ssl_st {
    mbedtls_ssl_context m;
    SSL_CTX *ctx;
    SSL_CTX *initial_ctx;
    struct myos_conf *conf;
    int fd;
    int server;
    int set_up;
    int handshaking;
    int handshake_done;
    int shutdown;
    int quiet_shutdown;
    int error;
    int sys_errno;
    unsigned long options;
    int verify_mode;
    int verify_depth;
    SSL_verify_cb verify_cb;
    long verify_result;
    char *servername;
    char *hostname;
    SSL_SESSION *session;
    int reused;
    X509 *own_x509;
    X509 *own_pending;
    mbedtls_x509_crt own_crt;
    BIO bio;
    SSL_CIPHER cipher;
    void *ex[MYOS_EX_SLOTS];
};

/* myos_crypto.c */
void myos_err_put(int lib, int reason, const char *data);
void myos_err_mbedtls(int lib, int reason, int ret);
int myos_rng(void *p, unsigned char *out, size_t len);
mbedtls_ctr_drbg_context *myos_drbg(void);
int myos_ex_index(int kind);
BIO *myos_bio_mem(void);
int myos_bio_append(BIO *b, const void *data, size_t len);
int myos_bio_puts(BIO *b, const char *s);

/* myos_x509.c */
X509 *myos_x509_from_der(const unsigned char *der, size_t len);
long myos_verify_flags_to_x509(uint32_t flags, const mbedtls_x509_crt *crt);

/* myos_ssl.c: the connection a session cache callback is for. */
SSL *myos_ssl_current(void);

#endif /* MYOS_SSL_INT_H */
