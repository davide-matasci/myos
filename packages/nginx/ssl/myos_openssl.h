/* The part of the OpenSSL 1.1.1 API nginx uses, over mbedtls (ports/mbedtls,
 * the TLS library curl, lynx and http use): every <openssl/...> header of
 * this directory includes this one, myos_ssl.c and myos_x509.c implement it.
 *
 * TLS 1.2 (mbedtls's TLS 1.3 needs its PSA crypto, which the myos build
 * leaves out), servers and clients, certificates and keys in PEM, SNI,
 * client certificates, the session cache's callbacks. What nginx can do
 * without is declared absent the way OpenSSL builds declare it, so nginx
 * leaves it out itself: OCSP, engines, DH parameters (ECDHE only), ALPN,
 * session ticket keys, early data, the ClientHello and certificate
 * callbacks (variables in ssl_certificate). */
#ifndef MYOS_OPENSSL_H
#define MYOS_OPENSSL_H

#include <stddef.h>
#include <stdint.h>
#include <time.h>

#define OPENSSL_VERSION_NUMBER  0x1010117fL
#define OPENSSL_VERSION_TEXT    "OpenSSL API over Mbed TLS (myos)"
#define OPENSSL_VERSION         0
#define OPENSSL_NO_OCSP
#define OPENSSL_NO_ENGINE
#define OPENSSL_NO_DH
#define OPENSSL_NO_COMP

/* ---- types --------------------------------------------------------- */

typedef struct ssl_ctx_st SSL_CTX;
typedef struct ssl_st SSL;
typedef struct ssl_session_st SSL_SESSION;
typedef struct ssl_method_st SSL_METHOD;
typedef struct ssl_cipher_st SSL_CIPHER;
typedef struct x509_st X509;
typedef struct x509_name_st X509_NAME;
typedef struct x509_name_entry_st X509_NAME_ENTRY;
typedef struct x509_store_st X509_STORE;
typedef struct x509_store_ctx_st X509_STORE_CTX;
typedef struct x509_crl_st X509_CRL;
typedef struct evp_pkey_st EVP_PKEY;
typedef struct evp_md_st EVP_MD;
typedef struct evp_md_ctx_st EVP_MD_CTX;
typedef struct bio_st BIO;
typedef struct asn1_string_st ASN1_STRING;
typedef struct asn1_string_st ASN1_INTEGER;
typedef struct asn1_string_st ASN1_TIME;
typedef struct asn1_string_st ASN1_IA5STRING;
typedef struct general_name_st GENERAL_NAME;
typedef struct ec_key_st EC_KEY;
typedef struct ssl_comp_st SSL_COMP;
typedef struct ssl_conf_ctx_st SSL_CONF_CTX;
typedef struct crypto_ex_data_st CRYPTO_EX_DATA;
typedef int pem_password_cb(char *buf, int size, int rwflag, void *userdata);
typedef void CRYPTO_EX_new(void *parent, void *ptr, CRYPTO_EX_DATA *ad, int idx,
    long argl, void *argp);
typedef void CRYPTO_EX_free(void *parent, void *ptr, CRYPTO_EX_DATA *ad, int idx,
    long argl, void *argp);
typedef int CRYPTO_EX_dup(CRYPTO_EX_DATA *to, const CRYPTO_EX_DATA *from,
    void *from_d, int idx, long argl, void *argp);

/* ---- stacks -------------------------------------------------------- */

typedef struct openssl_stack_st OPENSSL_STACK;
#define STACK_OF(type) struct stack_st_##type

OPENSSL_STACK *OPENSSL_sk_new_null(void);
OPENSSL_STACK *OPENSSL_sk_dup(const OPENSSL_STACK *st);
int OPENSSL_sk_num(const OPENSSL_STACK *st);
void *OPENSSL_sk_value(const OPENSSL_STACK *st, int i);
int OPENSSL_sk_push(OPENSSL_STACK *st, const void *data);
void *OPENSSL_sk_shift(OPENSSL_STACK *st);
void *OPENSSL_sk_pop(OPENSSL_STACK *st);
int OPENSSL_sk_find(OPENSSL_STACK *st, const void *data);
void OPENSSL_sk_free(OPENSSL_STACK *st);
void OPENSSL_sk_pop_free(OPENSSL_STACK *st, void (*func)(void *));
OPENSSL_STACK *OPENSSL_sk_new(int (*cmp)(const void *, const void *));

/* sk_T_*: the typed names of OPENSSL_sk_* (OpenSSL's DEFINE_STACK_OF). */
#define MYOS_DEFINE_STACK_OF(T) \
    STACK_OF(T); \
    static inline STACK_OF(T) *sk_##T##_new_null(void) \
        { return (STACK_OF(T) *)OPENSSL_sk_new_null(); } \
    static inline STACK_OF(T) *sk_##T##_new(int (*cmp)(const T *const *, const T *const *)) \
        { return (STACK_OF(T) *)OPENSSL_sk_new((int (*)(const void *, const void *))cmp); } \
    static inline STACK_OF(T) *sk_##T##_dup(const STACK_OF(T) *st) \
        { return (STACK_OF(T) *)OPENSSL_sk_dup((const OPENSSL_STACK *)st); } \
    static inline int sk_##T##_num(const STACK_OF(T) *st) \
        { return OPENSSL_sk_num((const OPENSSL_STACK *)st); } \
    static inline T *sk_##T##_value(const STACK_OF(T) *st, int i) \
        { return (T *)OPENSSL_sk_value((const OPENSSL_STACK *)st, i); } \
    static inline int sk_##T##_push(STACK_OF(T) *st, T *v) \
        { return OPENSSL_sk_push((OPENSSL_STACK *)st, v); } \
    static inline T *sk_##T##_shift(STACK_OF(T) *st) \
        { return (T *)OPENSSL_sk_shift((OPENSSL_STACK *)st); } \
    static inline T *sk_##T##_pop(STACK_OF(T) *st) \
        { return (T *)OPENSSL_sk_pop((OPENSSL_STACK *)st); } \
    static inline int sk_##T##_find(STACK_OF(T) *st, T *v) \
        { return OPENSSL_sk_find((OPENSSL_STACK *)st, v); } \
    static inline void sk_##T##_free(STACK_OF(T) *st) \
        { OPENSSL_sk_free((OPENSSL_STACK *)st); } \
    static inline void sk_##T##_pop_free(STACK_OF(T) *st, void (*f)(T *)) \
        { OPENSSL_sk_pop_free((OPENSSL_STACK *)st, (void (*)(void *))f); }

MYOS_DEFINE_STACK_OF(X509)
MYOS_DEFINE_STACK_OF(X509_NAME)
MYOS_DEFINE_STACK_OF(X509_CRL)
MYOS_DEFINE_STACK_OF(GENERAL_NAME)
MYOS_DEFINE_STACK_OF(SSL_CIPHER)
MYOS_DEFINE_STACK_OF(SSL_COMP)
typedef STACK_OF(GENERAL_NAME) GENERAL_NAMES;

/* ---- memory, init, version ----------------------------------------- */

void *CRYPTO_malloc(size_t num, const char *file, int line);
void CRYPTO_free(void *ptr, const char *file, int line);
#define OPENSSL_malloc(n) CRYPTO_malloc(n, __FILE__, __LINE__)
#define OPENSSL_free(p) CRYPTO_free(p, __FILE__, __LINE__)
const char *OpenSSL_version(int type);
#define SSLeay_version(t) OpenSSL_version(t)
#define SSLEAY_VERSION OPENSSL_VERSION
#define OPENSSL_config(name) ((void)(name))
#define OPENSSL_no_config() ((void)0)
#define SSL_library_init() 1
#define SSL_load_error_strings() ((void)0)
#define OpenSSL_add_all_algorithms() ((void)0)
int RAND_bytes(unsigned char *buf, int num);

/* ---- errors -------------------------------------------------------- */

#define ERR_LIB_SYS 2
#define ERR_LIB_PEM 9
#define ERR_LIB_X509 11
#define ERR_LIB_EVP 6
#define ERR_LIB_SSL 20
#define ERR_TXT_MALLOCED 0x01
#define ERR_TXT_STRING 0x02
#define ERR_PACK(lib, func, reason) \
    ((((unsigned long)(lib) & 0xffUL) << 24) | ((unsigned long)(reason) & 0xfffUL))
#define ERR_GET_LIB(e) ((int)(((unsigned long)(e) >> 24) & 0xffUL))
#define ERR_GET_REASON(e) ((int)((unsigned long)(e) & 0xfffUL))

unsigned long ERR_get_error(void);
unsigned long ERR_peek_error(void);
unsigned long ERR_peek_last_error(void);
unsigned long ERR_peek_error_line_data(const char **file, int *line,
    const char **data, int *flags);
void ERR_clear_error(void);
void ERR_error_string_n(unsigned long e, char *buf, size_t len);

#define PEM_R_NO_START_LINE 108
#define X509_R_CERT_ALREADY_IN_HASH_TABLE 101
#define X509_R_KEY_VALUES_MISMATCH 116

/* The reasons nginx tells a client's errors by (ngx_ssl_connection_error),
 * OpenSSL's numbers. */
#define SSL_R_BAD_CHANGE_CIPHER_SPEC 103
#define SSL_R_BAD_DIGEST_LENGTH 111
#define SSL_R_BAD_PACKET_LENGTH 115
#define SSL_R_BLOCK_CIPHER_PAD_IS_WRONG 129
#define SSL_R_CCS_RECEIVED_EARLY 133
#define SSL_R_DATA_LENGTH_TOO_LONG 146
#define SSL_R_DIGEST_CHECK_FAILED 149
#define SSL_R_ENCRYPTED_LENGTH_TOO_LONG 150
#define SSL_R_ERROR_IN_RECEIVED_CIPHER_LIST 151
#define SSL_R_EXCESSIVE_MESSAGE_SIZE 152
#define SSL_R_HTTPS_PROXY_REQUEST 155
#define SSL_R_HTTP_REQUEST 156
#define SSL_R_LENGTH_MISMATCH 159
#define SSL_R_NO_CIPHERS_SPECIFIED 183
#define SSL_R_NO_COMPRESSION_SPECIFIED 187
#define SSL_R_NO_SHARED_CIPHER 193
#define SSL_R_RECORD_LENGTH_MISMATCH 213
#define SSL_R_UNEXPECTED_MESSAGE 244
#define SSL_R_UNEXPECTED_RECORD 245
#define SSL_R_UNKNOWN_ALERT_TYPE 246
#define SSL_R_UNKNOWN_PROTOCOL 252
#define SSL_R_UNSUPPORTED_PROTOCOL 258
#define SSL_R_WRONG_VERSION_NUMBER 267
#define SSL_R_BAD_LENGTH 271
#define SSL_R_DECRYPTION_FAILED_OR_BAD_RECORD_MAC 281
#define SSL_R_CERTIFICATE_VERIFY_FAILED 134
#define SSL_R_NO_CERTIFICATE_SET 179
#define SSL_AD_REASON_OFFSET 1000

/* ---- BIO ----------------------------------------------------------- */

typedef struct bio_method_st BIO_METHOD;
const BIO_METHOD *BIO_s_mem(void);
BIO *BIO_new(const BIO_METHOD *type);
BIO *BIO_new_file(const char *filename, const char *mode);
BIO *BIO_new_mem_buf(const void *buf, int len);
int BIO_free(BIO *a);
int BIO_read(BIO *b, void *data, int dlen);
int BIO_write(BIO *b, const void *data, int dlen);
long BIO_ctrl(BIO *bp, int cmd, long larg, void *parg);
#define BIO_CTRL_RESET 1
#define BIO_CTRL_INFO 3
#define BIO_CTRL_PENDING 10
#define BIO_C_SET_BUFF_SIZE 117
#define BIO_reset(b) (int)BIO_ctrl(b, BIO_CTRL_RESET, 0, NULL)
#define BIO_pending(b) (int)BIO_ctrl(b, BIO_CTRL_PENDING, 0, NULL)
#define BIO_get_mem_data(b, pp) BIO_ctrl(b, BIO_CTRL_INFO, 0, (char *)(pp))
#define BIO_set_write_buffer_size(b, size) BIO_ctrl(b, BIO_C_SET_BUFF_SIZE, size, NULL)

/* ---- ASN.1 --------------------------------------------------------- */

#define V_ASN1_IA5STRING 22
const unsigned char *ASN1_STRING_get0_data(const ASN1_STRING *x);
int ASN1_STRING_length(const ASN1_STRING *x);
int ASN1_TIME_print(BIO *fp, const ASN1_TIME *a);
int i2a_ASN1_INTEGER(BIO *bp, const ASN1_INTEGER *a);
time_t myos_asn1_time_t(const ASN1_TIME *a);

/* ---- objects ------------------------------------------------------- */

#define NID_undef 0
#define NID_commonName 13
#define NID_subject_alt_name 85
#define NID_X9_62_prime256v1 415
#define NID_secp384r1 715
const char *OBJ_nid2sn(int n);
int OBJ_sn2nid(const char *s);

/* ---- EVP: keys and digests ----------------------------------------- */

#define EVP_MAX_MD_SIZE 64
EVP_PKEY *EVP_PKEY_new(void);
void EVP_PKEY_free(EVP_PKEY *pkey);
int EVP_PKEY_up_ref(EVP_PKEY *pkey);
const EVP_MD *EVP_sha1(void);
const EVP_MD *EVP_sha256(void);
EVP_MD_CTX *EVP_MD_CTX_new(void);
void EVP_MD_CTX_free(EVP_MD_CTX *ctx);
int EVP_DigestInit_ex(EVP_MD_CTX *ctx, const EVP_MD *type, void *impl);
int EVP_DigestUpdate(EVP_MD_CTX *ctx, const void *d, size_t cnt);
int EVP_DigestFinal_ex(EVP_MD_CTX *ctx, unsigned char *md, unsigned int *s);
#define EVP_MD_CTX_create EVP_MD_CTX_new
#define EVP_MD_CTX_destroy EVP_MD_CTX_free

/* ---- PEM ----------------------------------------------------------- */

X509 *PEM_read_bio_X509(BIO *bp, X509 **x, pem_password_cb *cb, void *u);
X509 *PEM_read_bio_X509_AUX(BIO *bp, X509 **x, pem_password_cb *cb, void *u);
X509_CRL *PEM_read_bio_X509_CRL(BIO *bp, X509_CRL **x, pem_password_cb *cb, void *u);
EVP_PKEY *PEM_read_bio_PrivateKey(BIO *bp, EVP_PKEY **x, pem_password_cb *cb, void *u);
int PEM_write_bio_X509(BIO *bp, X509 *x);

/* ---- X509 ---------------------------------------------------------- */

#define X509_V_OK 0
#define X509_V_ERR_UNABLE_TO_GET_ISSUER_CERT 2
#define X509_V_ERR_UNABLE_TO_GET_CRL 3
#define X509_V_ERR_CERT_SIGNATURE_FAILURE 7
#define X509_V_ERR_CERT_NOT_YET_VALID 9
#define X509_V_ERR_CERT_HAS_EXPIRED 10
#define X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT 18
#define X509_V_ERR_SELF_SIGNED_CERT_IN_CHAIN 19
#define X509_V_ERR_UNABLE_TO_GET_ISSUER_CERT_LOCALLY 20
#define X509_V_ERR_UNABLE_TO_VERIFY_LEAF_SIGNATURE 21
#define X509_V_ERR_CERT_CHAIN_TOO_LONG 22
#define X509_V_ERR_CERT_REVOKED 23
#define X509_V_ERR_INVALID_PURPOSE 26
#define X509_V_ERR_CERT_UNTRUSTED 27
#define X509_V_ERR_CERT_REJECTED 28
#define X509_V_ERR_HOSTNAME_MISMATCH 62
#define X509_V_ERR_APPLICATION_VERIFICATION 50
#define X509_V_FLAG_CRL_CHECK 0x4
#define X509_V_FLAG_CRL_CHECK_ALL 0x8
#define XN_FLAG_RFC2253 0x1172031UL
#define X509_CHECK_FLAG_ALWAYS_CHECK_SUBJECT 0x1
#define GEN_DNS 2

struct general_name_st {
    int type;
    union {
        ASN1_IA5STRING *dNSName;
        ASN1_IA5STRING *ia5;
    } d;
};

void X509_free(X509 *a);
int X509_up_ref(X509 *a);
X509_NAME *X509_get_subject_name(const X509 *a);
X509_NAME *X509_get_issuer_name(const X509 *a);
ASN1_INTEGER *X509_get_serialNumber(X509 *x);
const ASN1_TIME *X509_get0_notBefore(const X509 *x);
const ASN1_TIME *X509_get0_notAfter(const X509 *x);
#define X509_get_notBefore(x) ((ASN1_TIME *)X509_get0_notBefore(x))
#define X509_get_notAfter(x) ((ASN1_TIME *)X509_get0_notAfter(x))
int X509_digest(const X509 *data, const EVP_MD *type, unsigned char *md, unsigned int *len);
void *X509_get_ext_d2i(const X509 *x, int nid, int *crit, int *idx);
int X509_check_host(X509 *x, const char *chk, size_t chklen, unsigned int flags,
    char **peername);
int X509_check_issued(X509 *issuer, X509 *subject);
int X509_get_ex_new_index(long argl, void *argp, CRYPTO_EX_new *new_func,
    CRYPTO_EX_dup *dup_func, CRYPTO_EX_free *free_func);
int X509_set_ex_data(X509 *r, int idx, void *arg);
void *X509_get_ex_data(const X509 *r, int idx);
void GENERAL_NAMES_free(GENERAL_NAMES *a);
const char *X509_verify_cert_error_string(long n);

X509_NAME *X509_NAME_dup(const X509_NAME *xn);
void X509_NAME_free(X509_NAME *a);
int X509_NAME_cmp(const X509_NAME *a, const X509_NAME *b);
char *X509_NAME_oneline(const X509_NAME *a, char *buf, int size);
int X509_NAME_print_ex(BIO *out, const X509_NAME *nm, int indent, unsigned long flags);
int X509_NAME_digest(const X509_NAME *data, const EVP_MD *type, unsigned char *md,
    unsigned int *len);
int X509_NAME_get_index_by_NID(const X509_NAME *name, int nid, int lastpos);
X509_NAME_ENTRY *X509_NAME_get_entry(const X509_NAME *name, int loc);
ASN1_STRING *X509_NAME_ENTRY_get_data(const X509_NAME_ENTRY *ne);

void X509_CRL_free(X509_CRL *a);
int X509_CRL_up_ref(X509_CRL *a);

X509_STORE *X509_STORE_new(void);
void X509_STORE_free(X509_STORE *v);
int X509_STORE_add_cert(X509_STORE *ctx, X509 *x);
int X509_STORE_add_crl(X509_STORE *ctx, X509_CRL *x);
int X509_STORE_set_flags(X509_STORE *ctx, unsigned long flags);

void *X509_STORE_CTX_get_ex_data(const X509_STORE_CTX *ctx, int idx);
X509 *X509_STORE_CTX_get_current_cert(const X509_STORE_CTX *ctx);
int X509_STORE_CTX_get_error(const X509_STORE_CTX *ctx);
int X509_STORE_CTX_get_error_depth(const X509_STORE_CTX *ctx);

/* ---- SSL ----------------------------------------------------------- */

#define SSL2_VERSION 0x0002
#define SSL3_VERSION 0x0300
#define TLS1_VERSION 0x0301
#define TLS1_1_VERSION 0x0302
#define TLS1_2_VERSION 0x0303

#define SSL_ERROR_NONE 0
#define SSL_ERROR_SSL 1
#define SSL_ERROR_WANT_READ 2
#define SSL_ERROR_WANT_WRITE 3
#define SSL_ERROR_WANT_X509_LOOKUP 4
#define SSL_ERROR_SYSCALL 5
#define SSL_ERROR_ZERO_RETURN 6

#define SSL_OP_ALL 0x80000854UL
#define SSL_OP_NO_TICKET 0x00004000UL
#define SSL_OP_CIPHER_SERVER_PREFERENCE 0x00400000UL
#define SSL_OP_NO_SSLv2 0x0UL
#define SSL_OP_NO_SSLv3 0x02000000UL
#define SSL_OP_NO_TLSv1 0x04000000UL
#define SSL_OP_NO_TLSv1_2 0x08000000UL
#define SSL_OP_NO_TLSv1_1 0x10000000UL
#define SSL_OP_NO_COMPRESSION 0x00020000UL
#define SSL_OP_SINGLE_DH_USE 0x0UL
#define SSL_OP_SINGLE_ECDH_USE 0x0UL
#define SSL_OP_NO_RENEGOTIATION 0x40000000UL

#define SSL_VERIFY_NONE 0x00
#define SSL_VERIFY_PEER 0x01
#define SSL_VERIFY_FAIL_IF_NO_PEER_CERT 0x02
#define SSL_VERIFY_CLIENT_ONCE 0x04

#define SSL_SESS_CACHE_OFF 0x0000
#define SSL_SESS_CACHE_CLIENT 0x0001
#define SSL_SESS_CACHE_SERVER 0x0002
#define SSL_SESS_CACHE_NO_AUTO_CLEAR 0x0080
#define SSL_SESS_CACHE_NO_INTERNAL_LOOKUP 0x0100
#define SSL_SESS_CACHE_NO_INTERNAL_STORE 0x0200
#define SSL_SESS_CACHE_NO_INTERNAL \
    (SSL_SESS_CACHE_NO_INTERNAL_LOOKUP | SSL_SESS_CACHE_NO_INTERNAL_STORE)
#define SSL_MAX_SSL_SESSION_ID_LENGTH 32
#define SSL_MAX_SID_CTX_LENGTH 32

#define SSL_SENT_SHUTDOWN 1
#define SSL_RECEIVED_SHUTDOWN 2

#define SSL_ST_CONNECT 0x1000
#define SSL_ST_ACCEPT 0x2000
#define SSL_CB_LOOP 0x01
#define SSL_CB_EXIT 0x02
#define SSL_CB_READ 0x04
#define SSL_CB_WRITE 0x08
#define SSL_CB_ALERT 0x4000
#define SSL_CB_HANDSHAKE_START 0x10
#define SSL_CB_HANDSHAKE_DONE 0x20
#define SSL_CB_ACCEPT_LOOP (SSL_ST_ACCEPT | SSL_CB_LOOP)
#define SSL_CB_ACCEPT_EXIT (SSL_ST_ACCEPT | SSL_CB_EXIT)

#define SSL_AD_NO_RENEGOTIATION 100
#define SSL_AD_UNRECOGNIZED_NAME 112
#define SSL_AD_INTERNAL_ERROR 80
#define SSL_TLSEXT_ERR_OK 0
#define SSL_TLSEXT_ERR_ALERT_WARNING 1
#define SSL_TLSEXT_ERR_ALERT_FATAL 2
#define SSL_TLSEXT_ERR_NOACK 3
#define TLSEXT_NAMETYPE_host_name 0
#define SSL_CTRL_SET_TLSEXT_HOSTNAME 55

#define SSL_CONF_FLAG_FILE 0x2
#define SSL_CONF_FLAG_SERVER 0x8
#define SSL_CONF_FLAG_CLIENT 0x4
#define SSL_CONF_FLAG_CERTIFICATE 0x20
#define SSL_CONF_FLAG_SHOW_ERRORS 0x10
#define SSL_CONF_TYPE_UNKNOWN 0x0
#define SSL_CONF_TYPE_FILE 0x2
#define SSL_CONF_TYPE_DIR 0x3

typedef int (*SSL_verify_cb)(int preverify_ok, X509_STORE_CTX *x509_ctx);

const SSL_METHOD *TLS_method(void);
#define SSLv23_method TLS_method
SSL_CTX *SSL_CTX_new(const SSL_METHOD *meth);
void SSL_CTX_free(SSL_CTX *ctx);
int SSL_CTX_up_ref(SSL_CTX *ctx);
int SSL_CTX_get_ex_new_index(long argl, void *argp, CRYPTO_EX_new *new_func,
    CRYPTO_EX_dup *dup_func, CRYPTO_EX_free *free_func);
int SSL_CTX_set_ex_data(SSL_CTX *ctx, int idx, void *data);
void *SSL_CTX_get_ex_data(const SSL_CTX *ctx, int idx);
unsigned long SSL_CTX_set_options(SSL_CTX *ctx, unsigned long op);
unsigned long SSL_CTX_clear_options(SSL_CTX *ctx, unsigned long op);
unsigned long SSL_CTX_get_options(const SSL_CTX *ctx);
int SSL_CTX_set_min_proto_version(SSL_CTX *ctx, int version);
int SSL_CTX_set_max_proto_version(SSL_CTX *ctx, int version);
#define SSL_CTX_set_read_ahead(ctx, m) ((void)(ctx), (void)(m))
void SSL_CTX_set_info_callback(SSL_CTX *ctx, void (*cb)(const SSL *ssl, int type, int val));
int SSL_CTX_set_cipher_list(SSL_CTX *ctx, const char *str);
int SSL_CTX_use_certificate(SSL_CTX *ctx, X509 *x);
int SSL_CTX_use_PrivateKey(SSL_CTX *ctx, EVP_PKEY *pkey);
int SSL_CTX_set0_chain(SSL_CTX *ctx, STACK_OF(X509) *chain);
#define SSL_CTX_set0_chain SSL_CTX_set0_chain
X509_STORE *SSL_CTX_get_cert_store(const SSL_CTX *ctx);
void SSL_CTX_set_verify(SSL_CTX *ctx, int mode, SSL_verify_cb callback);
int SSL_CTX_get_verify_mode(const SSL_CTX *ctx);
SSL_verify_cb SSL_CTX_get_verify_callback(const SSL_CTX *ctx);
void SSL_CTX_set_verify_depth(SSL_CTX *ctx, int depth);
int SSL_CTX_get_verify_depth(const SSL_CTX *ctx);
void SSL_CTX_set_client_CA_list(SSL_CTX *ctx, STACK_OF(X509_NAME) *name_list);
STACK_OF(X509_NAME) *SSL_CTX_get_client_CA_list(const SSL_CTX *ctx);
long SSL_CTX_set_timeout(SSL_CTX *ctx, long t);
long SSL_CTX_get_timeout(const SSL_CTX *ctx);
long SSL_CTX_set_session_cache_mode(SSL_CTX *ctx, long mode);
long SSL_CTX_sess_set_cache_size(SSL_CTX *ctx, long size);
int SSL_CTX_set_session_id_context(SSL_CTX *ctx, const unsigned char *sid_ctx,
    unsigned int sid_ctx_len);
void SSL_CTX_sess_set_new_cb(SSL_CTX *ctx, int (*new_session_cb)(SSL *, SSL_SESSION *));
void SSL_CTX_sess_set_remove_cb(SSL_CTX *ctx,
    void (*remove_session_cb)(SSL_CTX *ctx, SSL_SESSION *));
void SSL_CTX_sess_set_get_cb(SSL_CTX *ctx, SSL_SESSION *(*get_session_cb)(SSL *,
    const unsigned char *, int, int *));
int SSL_CTX_remove_session(SSL_CTX *ctx, SSL_SESSION *session);
long SSL_CTX_set_tlsext_servername_callback(SSL_CTX *ctx,
    int (*cb)(SSL *s, int *ad, void *arg));
#define SSL_CTX_set_tlsext_servername_callback SSL_CTX_set_tlsext_servername_callback
int SSL_CTX_set1_curves_list(SSL_CTX *ctx, const char *list);
#define SSL_CTX_set1_curves_list SSL_CTX_set1_curves_list
#define SSL_CTX_set_ecdh_auto(ctx, onoff) 1

SSL_CONF_CTX *SSL_CONF_CTX_new(void);
void SSL_CONF_CTX_free(SSL_CONF_CTX *cctx);
unsigned int SSL_CONF_CTX_set_flags(SSL_CONF_CTX *cctx, unsigned int flags);
void SSL_CONF_CTX_set_ssl_ctx(SSL_CONF_CTX *cctx, SSL_CTX *ctx);
int SSL_CONF_cmd(SSL_CONF_CTX *cctx, const char *cmd, const char *value);
int SSL_CONF_cmd_value_type(SSL_CONF_CTX *cctx, const char *cmd);
int SSL_CONF_CTX_finish(SSL_CONF_CTX *cctx);

SSL *SSL_new(SSL_CTX *ctx);
void SSL_free(SSL *ssl);
int SSL_set_fd(SSL *s, int fd);
void SSL_set_connect_state(SSL *s);
void SSL_set_accept_state(SSL *s);
int SSL_is_server(const SSL *s);
int SSL_get_ex_new_index(long argl, void *argp, CRYPTO_EX_new *new_func,
    CRYPTO_EX_dup *dup_func, CRYPTO_EX_free *free_func);
int SSL_set_ex_data(SSL *ssl, int idx, void *data);
void *SSL_get_ex_data(const SSL *ssl, int idx);
int SSL_get_ex_data_X509_STORE_CTX_idx(void);
int SSL_do_handshake(SSL *s);
int SSL_read(SSL *ssl, void *buf, int num);
int SSL_write(SSL *ssl, const void *buf, int num);
int SSL_shutdown(SSL *s);
int SSL_get_error(const SSL *s, int ret_code);
int SSL_in_init(const SSL *s);
int SSL_is_init_finished(const SSL *s);
void SSL_set_quiet_shutdown(SSL *ssl, int mode);
void SSL_set_shutdown(SSL *ssl, int mode);
int SSL_get_shutdown(const SSL *ssl);
BIO *SSL_get_rbio(const SSL *s);
BIO *SSL_get_wbio(const SSL *s);
SSL_CTX *SSL_get_SSL_CTX(const SSL *ssl);
SSL_CTX *SSL_set_SSL_CTX(SSL *ssl, SSL_CTX *ctx);
unsigned long SSL_set_options(SSL *s, unsigned long op);
unsigned long SSL_clear_options(SSL *s, unsigned long op);
unsigned long SSL_get_options(const SSL *s);
void SSL_set_verify(SSL *s, int mode, SSL_verify_cb callback);
void SSL_set_verify_depth(SSL *s, int depth);
long SSL_get_verify_result(const SSL *ssl);
X509 *SSL_get_peer_certificate(const SSL *s);
STACK_OF(X509) *SSL_get_peer_cert_chain(const SSL *s);
X509 *SSL_get_certificate(const SSL *ssl);
int SSL_use_certificate(SSL *ssl, X509 *x);
int SSL_use_PrivateKey(SSL *ssl, EVP_PKEY *pkey);
const char *SSL_get_servername(const SSL *s, const int type);
int SSL_set_tlsext_host_name(SSL *s, const char *name);
#define SSL_set_tlsext_host_name SSL_set_tlsext_host_name
const char *SSL_get_version(const SSL *s);
int SSL_version(const SSL *s);
const SSL_CIPHER *SSL_get_current_cipher(const SSL *s);
const char *SSL_CIPHER_get_name(const SSL_CIPHER *c);
#define SSL_get_cipher_name(s) SSL_CIPHER_get_name(SSL_get_current_cipher(s))
char *SSL_get_shared_ciphers(const SSL *s, char *buf, int size);
STACK_OF(SSL_CIPHER) *SSL_get_ciphers(const SSL *s);

SSL_SESSION *SSL_get1_session(SSL *ssl);
SSL_SESSION *SSL_get0_session(const SSL *s);
int SSL_set_session(SSL *to, SSL_SESSION *session);
int SSL_session_reused(const SSL *s);
void SSL_SESSION_free(SSL_SESSION *ses);
int SSL_SESSION_up_ref(SSL_SESSION *ses);
const unsigned char *SSL_SESSION_get_id(const SSL_SESSION *s, unsigned int *len);
long SSL_SESSION_get_time(const SSL_SESSION *s);
long SSL_SESSION_set_time(SSL_SESSION *s, long t);
long SSL_SESSION_get_timeout(const SSL_SESSION *s);
long SSL_SESSION_set_timeout(SSL_SESSION *s, long t);
int SSL_SESSION_set1_id_context(SSL_SESSION *s, const unsigned char *sid_ctx,
    unsigned int sid_ctx_len);
int i2d_SSL_SESSION(SSL_SESSION *in, unsigned char **pp);
SSL_SESSION *d2i_SSL_SESSION(SSL_SESSION **a, const unsigned char **pp, long length);

#endif /* MYOS_OPENSSL_H */
