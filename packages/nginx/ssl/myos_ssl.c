/* The OpenSSL API over mbedtls (myos_openssl.h): contexts, connections,
 * sessions and their cache. */
#define MBEDTLS_ALLOW_PRIVATE_ACCESS /* the alert a peer sent (in_msg) */

#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <time.h>
#include <unistd.h>

#include <mbedtls/error.h>
#include <mbedtls/net_sockets.h>
#include <mbedtls/ssl_ciphersuites.h>

#include "myos_ssl_int.h"

static SSL *current;

/* The connection mbedtls is working for: its session cache and
 * verification callbacks get no connection of their own. */
SSL *myos_ssl_current(void) {
    return current;
}

/* ---- ciphers -------------------------------------------------------- */

/* OpenSSL's names of the suites this mbedtls has (TLS 1.2, ECDHE and RSA
 * key exchange, AES), and mbedtls's (the IANA name, dashes). */
static const struct {
    const char *openssl;
    const char *mbedtls;
} cipher_names[] = {
    {"ECDHE-ECDSA-AES128-GCM-SHA256", "TLS-ECDHE-ECDSA-WITH-AES-128-GCM-SHA256"},
    {"ECDHE-RSA-AES128-GCM-SHA256", "TLS-ECDHE-RSA-WITH-AES-128-GCM-SHA256"},
    {"ECDHE-ECDSA-AES256-GCM-SHA384", "TLS-ECDHE-ECDSA-WITH-AES-256-GCM-SHA384"},
    {"ECDHE-RSA-AES256-GCM-SHA384", "TLS-ECDHE-RSA-WITH-AES-256-GCM-SHA384"},
    {"ECDHE-ECDSA-AES128-SHA256", "TLS-ECDHE-ECDSA-WITH-AES-128-CBC-SHA256"},
    {"ECDHE-RSA-AES128-SHA256", "TLS-ECDHE-RSA-WITH-AES-128-CBC-SHA256"},
    {"ECDHE-ECDSA-AES256-SHA384", "TLS-ECDHE-ECDSA-WITH-AES-256-CBC-SHA384"},
    {"ECDHE-RSA-AES256-SHA384", "TLS-ECDHE-RSA-WITH-AES-256-CBC-SHA384"},
    {"ECDHE-ECDSA-AES128-SHA", "TLS-ECDHE-ECDSA-WITH-AES-128-CBC-SHA"},
    {"ECDHE-RSA-AES128-SHA", "TLS-ECDHE-RSA-WITH-AES-128-CBC-SHA"},
    {"ECDHE-ECDSA-AES256-SHA", "TLS-ECDHE-ECDSA-WITH-AES-256-CBC-SHA"},
    {"ECDHE-RSA-AES256-SHA", "TLS-ECDHE-RSA-WITH-AES-256-CBC-SHA"},
    {"AES128-GCM-SHA256", "TLS-RSA-WITH-AES-128-GCM-SHA256"},
    {"AES256-GCM-SHA384", "TLS-RSA-WITH-AES-256-GCM-SHA384"},
    {"AES128-SHA256", "TLS-RSA-WITH-AES-128-CBC-SHA256"},
    {"AES256-SHA256", "TLS-RSA-WITH-AES-256-CBC-SHA256"},
    {"AES128-SHA", "TLS-RSA-WITH-AES-128-CBC-SHA"},
    {"AES256-SHA", "TLS-RSA-WITH-AES-256-CBC-SHA"},
};

#define NCIPHER_NAMES (int)(sizeof cipher_names / sizeof cipher_names[0])

/* A list of OpenSSL's: the suites it names, in its order; a keyword
 * (HIGH, DEFAULT, ECDHE, ...) means mbedtls's own list, which has only
 * strong suites; an exclusion (!, -) or an ordering (+, @) is not
 * followed. A suite of a name this mbedtls lacks is left out. */
int SSL_CTX_set_cipher_list(SSL_CTX *ctx, const char *str) {
    char *list = strdup(str), *tok, *save;
    int n = 0, keyword = 0;
    if (list == NULL) {
        return 0;
    }
    for (tok = strtok_r(list, ":, ", &save); tok != NULL; tok = strtok_r(NULL, ":, ", &save)) {
        int i, id;
        if (strchr("!-+@", tok[0]) != NULL) {
            continue;
        }
        for (i = 0; i < NCIPHER_NAMES; i++) {
            if (strcmp(tok, cipher_names[i].openssl) == 0 || strcmp(tok, cipher_names[i].mbedtls) == 0) {
                break;
            }
        }
        if (i == NCIPHER_NAMES) {
            keyword = 1;
            continue;
        }
        id = mbedtls_ssl_get_ciphersuite_id(cipher_names[i].mbedtls);
        if (id != 0 && n < (int)(sizeof ctx->ciphersuites / sizeof ctx->ciphersuites[0]) - 1) {
            ctx->ciphersuites[n++] = id;
        }
    }
    free(list);
    if (keyword) {
        n = 0;
    } else if (n == 0) {
        myos_err_put(ERR_LIB_SSL, SSL_R_NO_CIPHERS_SPECIFIED, "no cipher match");
        return 0;
    }
    ctx->ciphersuites[n] = 0;
    ctx->nciphersuites = n;
    return 1;
}

const SSL_CIPHER *SSL_get_current_cipher(const SSL *s) {
    SSL *m = (SSL *)s;
    if (!s->handshake_done) {
        return NULL;
    }
    m->cipher.id = mbedtls_ssl_get_ciphersuite_id_from_ssl(&m->m);
    return &m->cipher;
}

const char *SSL_CIPHER_get_name(const SSL_CIPHER *c) {
    const char *name;
    if (c == NULL) {
        return "(NONE)";
    }
    name = mbedtls_ssl_get_ciphersuite_name(c->id);
    for (int i = 0; i < NCIPHER_NAMES; i++) {
        if (strcmp(name, cipher_names[i].mbedtls) == 0) {
            return cipher_names[i].openssl;
        }
    }
    return name;
}

/* What the client offered is not kept: the suite chosen ($ssl_ciphers). */
char *SSL_get_shared_ciphers(const SSL *s, char *buf, int size) {
    const char *name = SSL_CIPHER_get_name(SSL_get_current_cipher(s));
    if (size <= 0) {
        return NULL;
    }
    strncpy(buf, name, (size_t)size - 1);
    buf[size - 1] = '\0';
    return buf;
}

/* The groups this mbedtls has; the others of OpenSSL's names are left out
 * of a list, which must keep one. */
int SSL_CTX_set1_curves_list(SSL_CTX *ctx, const char *list) {
    static const char *const absent[] = {"X25519", "X448", "secp521r1", "P-521", NULL};
    char *l = strdup(list), *tok, *save;
    int n = 0, ok = 1;
    if (l == NULL) {
        return 0;
    }
    for (tok = strtok_r(l, ":", &save); tok != NULL; tok = strtok_r(NULL, ":", &save)) {
        uint16_t g = 0;
        int known = 0;
        if (strcmp(tok, "prime256v1") == 0 || strcasecmp(tok, "P-256") == 0
            || strcmp(tok, "secp256r1") == 0) {
            g = MBEDTLS_SSL_IANA_TLS_GROUP_SECP256R1;
        } else if (strcmp(tok, "secp384r1") == 0 || strcasecmp(tok, "P-384") == 0) {
            g = MBEDTLS_SSL_IANA_TLS_GROUP_SECP384R1;
        }
        for (int i = 0; absent[i] != NULL; i++) {
            known |= strcasecmp(tok, absent[i]) == 0;
        }
        if (g != 0 && n < 7) {
            ctx->groups[n++] = g;
        } else if (g == 0 && !known) {
            ok = 0;
        }
    }
    free(l);
    if (!ok || n == 0) {
        myos_err_put(ERR_LIB_SSL, 0, "no supported curve in the list");
        return 0;
    }
    ctx->groups[n] = MBEDTLS_SSL_IANA_TLS_GROUP_NONE;
    ctx->ngroups = n;
    return 1;
}

/* ---- contexts ------------------------------------------------------- */

static const SSL_METHOD tls_method;

const SSL_METHOD *TLS_method(void) {
    return &tls_method;
}

SSL_CTX *SSL_CTX_new(const SSL_METHOD *meth) {
    SSL_CTX *ctx = calloc(1, sizeof *ctx);
    (void)meth;
    if (ctx == NULL) {
        return NULL;
    }
    ctx->refs = 1;
    ctx->verify_depth = 100;
    ctx->timeout = 300;
    ctx->cache_mode = SSL_SESS_CACHE_SERVER;
    ctx->max_version = TLS1_2_VERSION;
    ctx->store = X509_STORE_new();
    if (ctx->store == NULL || myos_drbg() == NULL) {
        SSL_CTX_free(ctx);
        return NULL;
    }
    return ctx;
}

static void conf_release(struct myos_conf *c) {
    if (c != NULL && --c->refs == 0) {
        mbedtls_ssl_config_free(&c->conf);
        free(c);
    }
}

/* A change to a context: the configurations made of it are made again at
 * the next SSL_new (nginx configures a context before it connects). */
static void ctx_changed(SSL_CTX *ctx) {
    for (int i = 0; i < 2; i++) {
        conf_release(ctx->conf[i]);
        ctx->conf[i] = NULL;
    }
}

static void cert_free(struct myos_cert *c) {
    X509_free(c->x509);
    sk_X509_pop_free(c->chain, X509_free);
    EVP_PKEY_free(c->pkey);
    if (c->built) {
        mbedtls_x509_crt_free(&c->crts);
    }
    memset(c, 0, sizeof *c);
}

void SSL_CTX_free(SSL_CTX *ctx) {
    if (ctx == NULL || --ctx->refs > 0) {
        return;
    }
    ctx_changed(ctx);
    for (int i = 0; i < ctx->ncerts; i++) {
        cert_free(&ctx->certs[i]);
    }
    X509_STORE_free(ctx->store);
    sk_X509_NAME_pop_free(ctx->client_ca, X509_NAME_free);
    free(ctx);
}

int SSL_CTX_up_ref(SSL_CTX *ctx) {
    ctx->refs++;
    return 1;
}

int SSL_CTX_get_ex_new_index(long argl, void *argp, CRYPTO_EX_new *new_func,
    CRYPTO_EX_dup *dup_func, CRYPTO_EX_free *free_func) {
    (void)argl;
    (void)argp;
    (void)new_func;
    (void)dup_func;
    (void)free_func;
    return myos_ex_index(1);
}

int SSL_CTX_set_ex_data(SSL_CTX *ctx, int idx, void *data) {
    if (idx < 0 || idx >= MYOS_EX_SLOTS) {
        return 0;
    }
    ctx->ex[idx] = data;
    return 1;
}

void *SSL_CTX_get_ex_data(const SSL_CTX *ctx, int idx) {
    return idx >= 0 && idx < MYOS_EX_SLOTS ? ctx->ex[idx] : NULL;
}

unsigned long SSL_CTX_set_options(SSL_CTX *ctx, unsigned long op) {
    ctx->options |= op;
    ctx_changed(ctx);
    return ctx->options;
}

unsigned long SSL_CTX_clear_options(SSL_CTX *ctx, unsigned long op) {
    ctx->options &= ~op;
    ctx_changed(ctx);
    return ctx->options;
}

unsigned long SSL_CTX_get_options(const SSL_CTX *ctx) {
    return ctx->options;
}

int SSL_CTX_set_min_proto_version(SSL_CTX *ctx, int version) {
    ctx->min_version = version;
    ctx_changed(ctx);
    return 1;
}

int SSL_CTX_set_max_proto_version(SSL_CTX *ctx, int version) {
    ctx->max_version = version;
    ctx_changed(ctx);
    return 1;
}

void SSL_CTX_set_info_callback(SSL_CTX *ctx, void (*cb)(const SSL *ssl, int type, int val)) {
    ctx->info_cb = cb;
}

/* The slot of a certificate's or key's type (RSA, EC). */
static int cert_slot(SSL_CTX *ctx, mbedtls_pk_type_t type, int add) {
    if (type == MBEDTLS_PK_ECDSA) {
        type = MBEDTLS_PK_ECKEY;
    }
    for (int i = 0; i < ctx->ncerts; i++) {
        mbedtls_pk_type_t t = mbedtls_pk_get_type(&ctx->certs[i].x509->crt.pk);
        if ((t == MBEDTLS_PK_ECDSA ? MBEDTLS_PK_ECKEY : t) == type) {
            return i;
        }
    }
    if (!add || ctx->ncerts == MYOS_CERT_SLOTS) {
        return -1;
    }
    return ctx->ncerts++;
}

int SSL_CTX_use_certificate(SSL_CTX *ctx, X509 *x) {
    int i = cert_slot(ctx, mbedtls_pk_get_type(&x->crt.pk), 1);
    struct myos_cert *c;
    if (i < 0) {
        myos_err_put(ERR_LIB_SSL, 0, "too many certificates");
        return 0;
    }
    c = &ctx->certs[i];
    X509_up_ref(x);
    if (c->x509 != NULL) {
        EVP_PKEY *k = c->pkey;
        c->pkey = NULL;
        cert_free(c);
        EVP_PKEY_free(k);
    }
    c->x509 = x;
    ctx->cur_cert = i;
    ctx_changed(ctx);
    return 1;
}

int SSL_CTX_set0_chain(SSL_CTX *ctx, STACK_OF(X509) *chain) {
    struct myos_cert *c = &ctx->certs[ctx->cur_cert];
    if (c->x509 == NULL) {
        myos_err_put(ERR_LIB_SSL, SSL_R_NO_CERTIFICATE_SET, NULL);
        return 0;
    }
    sk_X509_pop_free(c->chain, X509_free);
    c->chain = chain;
    ctx_changed(ctx);
    return 1;
}

int SSL_CTX_use_PrivateKey(SSL_CTX *ctx, EVP_PKEY *pkey) {
    int i = cert_slot(ctx, mbedtls_pk_get_type(&pkey->pk), 0);
    struct myos_cert *c;
    if (i < 0) {
        myos_err_put(ERR_LIB_SSL, SSL_R_NO_CERTIFICATE_SET, NULL);
        return 0;
    }
    c = &ctx->certs[i];
    if (mbedtls_pk_check_pair(&c->x509->crt.pk, &pkey->pk, myos_rng, NULL) != 0) {
        myos_err_put(ERR_LIB_X509, X509_R_KEY_VALUES_MISMATCH, NULL);
        return 0;
    }
    EVP_PKEY_up_ref(pkey);
    EVP_PKEY_free(c->pkey);
    c->pkey = pkey;
    ctx_changed(ctx);
    return 1;
}

/* Read when the configurations are made, at the first SSL_new. */
X509_STORE *SSL_CTX_get_cert_store(const SSL_CTX *ctx) {
    return ctx->store;
}

void SSL_CTX_set_verify(SSL_CTX *ctx, int mode, SSL_verify_cb callback) {
    ctx->verify_mode = mode;
    ctx->verify_cb = callback;
    ctx_changed(ctx);
}

int SSL_CTX_get_verify_mode(const SSL_CTX *ctx) {
    return ctx->verify_mode;
}

SSL_verify_cb SSL_CTX_get_verify_callback(const SSL_CTX *ctx) {
    return ctx->verify_cb;
}

void SSL_CTX_set_verify_depth(SSL_CTX *ctx, int depth) {
    ctx->verify_depth = depth;
}

int SSL_CTX_get_verify_depth(const SSL_CTX *ctx) {
    return ctx->verify_depth;
}

/* The CertificateRequest's names are the trusted certificates' (mbedtls
 * sends those of the CA chain): the list is kept for nginx to read. */
void SSL_CTX_set_client_CA_list(SSL_CTX *ctx, STACK_OF(X509_NAME) *name_list) {
    sk_X509_NAME_pop_free(ctx->client_ca, X509_NAME_free);
    ctx->client_ca = name_list;
}

STACK_OF(X509_NAME) *SSL_CTX_get_client_CA_list(const SSL_CTX *ctx) {
    return ctx->client_ca;
}

long SSL_CTX_set_timeout(SSL_CTX *ctx, long t) {
    long old = ctx->timeout;
    ctx->timeout = t;
    return old;
}

long SSL_CTX_get_timeout(const SSL_CTX *ctx) {
    return ctx->timeout;
}

long SSL_CTX_set_session_cache_mode(SSL_CTX *ctx, long mode) {
    long old = ctx->cache_mode;
    ctx->cache_mode = mode;
    ctx_changed(ctx);
    return old;
}

/* There is no built-in cache: the callbacks' (nginx's shared one) only. */
long SSL_CTX_sess_set_cache_size(SSL_CTX *ctx, long size) {
    (void)ctx;
    (void)size;
    return 0;
}

int SSL_CTX_set_session_id_context(SSL_CTX *ctx, const unsigned char *sid_ctx,
    unsigned int sid_ctx_len) {
    if (sid_ctx_len > SSL_MAX_SID_CTX_LENGTH) {
        return 0;
    }
    memcpy(ctx->sid_ctx, sid_ctx, sid_ctx_len);
    ctx->sid_ctx_len = sid_ctx_len;
    return 1;
}

void SSL_CTX_sess_set_new_cb(SSL_CTX *ctx, int (*new_session_cb)(SSL *, SSL_SESSION *)) {
    ctx->new_session_cb = new_session_cb;
    ctx_changed(ctx);
}

void SSL_CTX_sess_set_remove_cb(SSL_CTX *ctx,
    void (*remove_session_cb)(SSL_CTX *ctx, SSL_SESSION *)) {
    ctx->remove_session_cb = remove_session_cb;
}

void SSL_CTX_sess_set_get_cb(SSL_CTX *ctx, SSL_SESSION *(*get_session_cb)(SSL *,
    const unsigned char *, int, int *)) {
    ctx->get_session_cb = get_session_cb;
    ctx_changed(ctx);
}

int SSL_CTX_remove_session(SSL_CTX *ctx, SSL_SESSION *session) {
    if (ctx->remove_session_cb != NULL && session != NULL) {
        ctx->remove_session_cb(ctx, session);
    }
    return 1;
}

long SSL_CTX_set_tlsext_servername_callback(SSL_CTX *ctx, int (*cb)(SSL *s, int *ad, void *arg)) {
    ctx->servername_cb = cb;
    return 1;
}

/* ---- SSL_CONF: no commands ------------------------------------------ */

SSL_CONF_CTX *SSL_CONF_CTX_new(void) {
    return calloc(1, sizeof(SSL_CONF_CTX));
}

void SSL_CONF_CTX_free(SSL_CONF_CTX *cctx) {
    free(cctx);
}

unsigned int SSL_CONF_CTX_set_flags(SSL_CONF_CTX *cctx, unsigned int flags) {
    return cctx->flags |= flags;
}

void SSL_CONF_CTX_set_ssl_ctx(SSL_CONF_CTX *cctx, SSL_CTX *ctx) {
    cctx->ctx = ctx;
}

/* ssl_conf_command: OpenSSL's commands are not mbedtls's. */
int SSL_CONF_cmd(SSL_CONF_CTX *cctx, const char *cmd, const char *value) {
    (void)cctx;
    (void)value;
    myos_err_put(ERR_LIB_SSL, 0, cmd);
    return -2;
}

int SSL_CONF_cmd_value_type(SSL_CONF_CTX *cctx, const char *cmd) {
    (void)cctx;
    (void)cmd;
    return SSL_CONF_TYPE_UNKNOWN;
}

int SSL_CONF_CTX_finish(SSL_CONF_CTX *cctx) {
    (void)cctx;
    return 1;
}

/* ---- sessions ------------------------------------------------------- */

static SSL_SESSION *session_new(void) {
    SSL_SESSION *s = calloc(1, sizeof *s);
    if (s != NULL) {
        s->refs = 1;
    }
    return s;
}

/* mbedtls's session, serialized, as OpenSSL's. */
static SSL_SESSION *session_from(const mbedtls_ssl_session *ms, const SSL_CTX *ctx) {
    SSL_SESSION *s = session_new();
    size_t len = 0;
    if (s == NULL) {
        return NULL;
    }
    if (mbedtls_ssl_session_save(ms, NULL, 0, &len) != MBEDTLS_ERR_SSL_BUFFER_TOO_SMALL
        || (s->data = malloc(len)) == NULL
        || mbedtls_ssl_session_save(ms, s->data, len, &s->data_len) != 0) {
        SSL_SESSION_free(s);
        return NULL;
    }
    s->id_len = (unsigned int)mbedtls_ssl_session_get_id_len(ms);
    if (s->id_len > sizeof s->id) {
        s->id_len = sizeof s->id;
    }
    memcpy(s->id, mbedtls_ssl_session_get_id(ms), s->id_len);
    s->time = (long)time(NULL);
    s->timeout = ctx->timeout;
    memcpy(s->sid_ctx, ctx->sid_ctx, ctx->sid_ctx_len);
    s->sid_ctx_len = ctx->sid_ctx_len;
    return s;
}

void SSL_SESSION_free(SSL_SESSION *ses) {
    if (ses != NULL && --ses->refs == 0) {
        if (ses->data != NULL) {
            mbedtls_platform_zeroize(ses->data, ses->data_len);
        }
        free(ses->data);
        free(ses);
    }
}

int SSL_SESSION_up_ref(SSL_SESSION *ses) {
    ses->refs++;
    return 1;
}

const unsigned char *SSL_SESSION_get_id(const SSL_SESSION *s, unsigned int *len) {
    if (len != NULL) {
        *len = s->id_len;
    }
    return s->id;
}

long SSL_SESSION_get_time(const SSL_SESSION *s) {
    return s->time;
}

long SSL_SESSION_set_time(SSL_SESSION *s, long t) {
    s->time = t;
    return t;
}

long SSL_SESSION_get_timeout(const SSL_SESSION *s) {
    return s->timeout;
}

long SSL_SESSION_set_timeout(SSL_SESSION *s, long t) {
    s->timeout = t;
    return 1;
}

int SSL_SESSION_set1_id_context(SSL_SESSION *s, const unsigned char *sid_ctx,
    unsigned int sid_ctx_len) {
    if (sid_ctx_len > SSL_MAX_SID_CTX_LENGTH) {
        return 0;
    }
    memcpy(s->sid_ctx, sid_ctx, sid_ctx_len);
    s->sid_ctx_len = sid_ctx_len;
    return 1;
}

/* The serialized session: "MS", the id, the context, the time and the
 * timeout, then mbedtls's own serialization of it. */
static void put_u64(unsigned char *p, uint64_t v) {
    for (int i = 0; i < 8; i++) {
        p[i] = (unsigned char)(v >> (56 - 8 * i));
    }
}

static uint64_t get_u64(const unsigned char *p) {
    uint64_t v = 0;
    for (int i = 0; i < 8; i++) {
        v = v << 8 | p[i];
    }
    return v;
}

int i2d_SSL_SESSION(SSL_SESSION *in, unsigned char **pp) {
    size_t len = 2 + 1 + in->id_len + 1 + in->sid_ctx_len + 8 + 8 + 8 + in->data_len;
    unsigned char *p;
    if (pp == NULL || *pp == NULL) {
        return (int)len;
    }
    p = *pp;
    *p++ = 'M';
    *p++ = 'S';
    *p++ = (unsigned char)in->id_len;
    memcpy(p, in->id, in->id_len);
    p += in->id_len;
    *p++ = (unsigned char)in->sid_ctx_len;
    memcpy(p, in->sid_ctx, in->sid_ctx_len);
    p += in->sid_ctx_len;
    put_u64(p, (uint64_t)in->time);
    put_u64(p + 8, (uint64_t)in->timeout);
    put_u64(p + 16, in->data_len);
    p += 24;
    memcpy(p, in->data, in->data_len);
    *pp = p + in->data_len;
    return (int)len;
}

SSL_SESSION *d2i_SSL_SESSION(SSL_SESSION **a, const unsigned char **pp, long length) {
    const unsigned char *p = *pp, *end = p + length;
    SSL_SESSION *s;
    uint64_t dlen;
    (void)a;
    if (length < 3 || p[0] != 'M' || p[1] != 'S' || p[2] > 32) {
        return NULL;
    }
    s = session_new();
    if (s == NULL) {
        return NULL;
    }
    p += 2;
    s->id_len = *p++;
    if (end - p < (long)s->id_len + 1) {
        goto bad;
    }
    memcpy(s->id, p, s->id_len);
    p += s->id_len;
    s->sid_ctx_len = *p++;
    if (s->sid_ctx_len > SSL_MAX_SID_CTX_LENGTH || end - p < (long)s->sid_ctx_len + 24) {
        goto bad;
    }
    memcpy(s->sid_ctx, p, s->sid_ctx_len);
    p += s->sid_ctx_len;
    s->time = (long)get_u64(p);
    s->timeout = (long)get_u64(p + 8);
    dlen = get_u64(p + 16);
    p += 24;
    if ((uint64_t)(end - p) < dlen || (s->data = malloc(dlen ? dlen : 1)) == NULL) {
        goto bad;
    }
    memcpy(s->data, p, dlen);
    s->data_len = dlen;
    *pp = p + dlen;
    return s;
bad:
    SSL_SESSION_free(s);
    return NULL;
}

/* mbedtls's server-side cache, nginx's callbacks behind it. */
static int cache_get(void *data, unsigned char const *id, size_t id_len,
    mbedtls_ssl_session *session) {
    SSL *s = current;
    SSL_CTX *ctx = s != NULL ? s->initial_ctx : data;
    SSL_SESSION *sess;
    int copy = 1, ret;
    if (ctx->get_session_cb == NULL || s == NULL) {
        return -1;
    }
    sess = ctx->get_session_cb(s, id, (int)id_len, &copy);
    if (sess == NULL) {
        return -1;
    }
    ret = -1;
    if (sess->sid_ctx_len == ctx->sid_ctx_len
        && memcmp(sess->sid_ctx, ctx->sid_ctx, ctx->sid_ctx_len) == 0
        && (long)time(NULL) - sess->time < sess->timeout) {
        ret = mbedtls_ssl_session_load(session, sess->data, sess->data_len);
    }
    if (ret == 0) {
        s->reused = 1;
    }
    if (!copy) {
        SSL_SESSION_free(sess);
    }
    return ret == 0 ? 0 : -1;
}

static int cache_set(void *data, unsigned char const *id, size_t id_len,
    const mbedtls_ssl_session *session) {
    SSL *s = current;
    SSL_CTX *ctx = s != NULL ? s->initial_ctx : data;
    SSL_SESSION *sess;
    (void)id;
    (void)id_len;
    if (ctx->new_session_cb == NULL || s == NULL || !(ctx->cache_mode & SSL_SESS_CACHE_SERVER)) {
        return 0;
    }
    sess = session_from(session, ctx);
    if (sess != NULL && !ctx->new_session_cb(s, sess)) {
        SSL_SESSION_free(sess);
    }
    return 0;
}

/* ---- configurations ------------------------------------------------- */

/* A slot's chain, as mbedtls wants it: the certificate, then the chain. */
static int cert_build(struct myos_cert *c) {
    int ret;
    if (c->built) {
        return 0;
    }
    mbedtls_x509_crt_init(&c->crts);
    c->built = 1;
    ret = mbedtls_x509_crt_parse_der(&c->crts, c->x509->crt.raw.p, c->x509->crt.raw.len);
    for (int i = 0; ret == 0 && i < sk_X509_num(c->chain); i++) {
        X509 *x = sk_X509_value(c->chain, i);
        ret = mbedtls_x509_crt_parse_der(&c->crts, x->crt.raw.p, x->crt.raw.len);
    }
    return ret;
}

static int authmode(int verify_mode, SSL_verify_cb cb) {
    if (!(verify_mode & SSL_VERIFY_PEER)) {
        return MBEDTLS_SSL_VERIFY_NONE;
    }
    /* With a callback (nginx's accepts every certificate) the handshake
     * goes on, and the verification's result is the connection's. */
    return cb != NULL ? MBEDTLS_SSL_VERIFY_OPTIONAL : MBEDTLS_SSL_VERIFY_REQUIRED;
}

/* The verification depth: OpenSSL's counts the certificates above the
 * peer's, mbedtls numbers the peer's 0. */
static int verify_cb(void *p, mbedtls_x509_crt *crt, int depth, uint32_t *flags) {
    SSL *s = current;
    (void)p;
    (void)crt;
    if (s != NULL && depth > s->verify_depth) {
        *flags |= MBEDTLS_X509_BADCERT_OTHER;
        s->verify_result = X509_V_ERR_CERT_CHAIN_TOO_LONG;
    }
    return 0;
}

static int sni_cb(void *p, mbedtls_ssl_context *m, const unsigned char *name, size_t len);

static struct myos_conf *conf_make(SSL_CTX *ctx, int server) {
    struct myos_conf *c = calloc(1, sizeof *c);
    int ret;
    if (c == NULL) {
        return NULL;
    }
    c->refs = 1;
    mbedtls_ssl_config_init(&c->conf);
    ret = mbedtls_ssl_config_defaults(&c->conf,
        server ? MBEDTLS_SSL_IS_SERVER : MBEDTLS_SSL_IS_CLIENT,
        MBEDTLS_SSL_TRANSPORT_STREAM, MBEDTLS_SSL_PRESET_DEFAULT);
    if (ret != 0) {
        goto fail;
    }
    mbedtls_ssl_conf_rng(&c->conf, myos_rng, NULL);
    mbedtls_ssl_conf_min_tls_version(&c->conf, MBEDTLS_SSL_VERSION_TLS1_2);
    mbedtls_ssl_conf_max_tls_version(&c->conf, MBEDTLS_SSL_VERSION_TLS1_2);
    mbedtls_ssl_conf_renegotiation(&c->conf, MBEDTLS_SSL_RENEGOTIATION_DISABLED);
    mbedtls_ssl_conf_authmode(&c->conf, authmode(ctx->verify_mode, ctx->verify_cb));
    mbedtls_ssl_conf_verify(&c->conf, verify_cb, NULL);
    if (ctx->store->ncas > 0) {
        mbedtls_ssl_conf_ca_chain(&c->conf, &ctx->store->cas,
            ctx->store->ncrls > 0 ? &ctx->store->crls : NULL);
    }
    if (ctx->nciphersuites > 0) {
        memcpy(c->ciphersuites, ctx->ciphersuites, sizeof c->ciphersuites);
        mbedtls_ssl_conf_ciphersuites(&c->conf, c->ciphersuites);
    }
    if (ctx->ngroups > 0) {
        memcpy(c->groups, ctx->groups, sizeof c->groups);
        mbedtls_ssl_conf_groups(&c->conf, c->groups);
    }
    for (int i = 0; i < ctx->ncerts; i++) {
        struct myos_cert *crt = &ctx->certs[i];
        if (crt->x509 == NULL || crt->pkey == NULL) {
            continue;
        }
        if ((ret = cert_build(crt)) != 0
            || (ret = mbedtls_ssl_conf_own_cert(&c->conf, &crt->crts, &crt->pkey->pk)) != 0) {
            goto fail;
        }
        if (!server) {
            break;
        }
    }
    if (server) {
        mbedtls_ssl_conf_preference_order(&c->conf,
            (ctx->options & SSL_OP_CIPHER_SERVER_PREFERENCE)
                ? MBEDTLS_SSL_SRV_CIPHERSUITE_ORDER_SERVER
                : MBEDTLS_SSL_SRV_CIPHERSUITE_ORDER_CLIENT);
        mbedtls_ssl_conf_sni(&c->conf, sni_cb, NULL);
        if ((ctx->cache_mode & SSL_SESS_CACHE_SERVER) && ctx->get_session_cb != NULL) {
            mbedtls_ssl_conf_session_cache(&c->conf, ctx, cache_get, cache_set);
        }
    }
    return c;
fail:
    myos_err_mbedtls(ERR_LIB_SSL, 0, ret);
    conf_release(c);
    return NULL;
}

/* ---- connections ---------------------------------------------------- */

SSL *SSL_new(SSL_CTX *ctx) {
    SSL *s = calloc(1, sizeof *s);
    if (s == NULL) {
        return NULL;
    }
    mbedtls_ssl_init(&s->m);
    s->ctx = s->initial_ctx = ctx;
    ctx->refs += 2;
    s->fd = -1;
    s->bio.fd = -1;
    s->options = ctx->options;
    s->verify_mode = ctx->verify_mode;
    s->verify_depth = ctx->verify_depth;
    s->verify_cb = ctx->verify_cb;
    return s;
}

void SSL_free(SSL *ssl) {
    if (ssl == NULL) {
        return;
    }
    mbedtls_ssl_free(&ssl->m);
    conf_release(ssl->conf);
    SSL_CTX_free(ssl->ctx);
    SSL_CTX_free(ssl->initial_ctx);
    SSL_SESSION_free(ssl->session);
    X509_free(ssl->own_x509);
    X509_free(ssl->own_pending);
    if (ssl->own_x509 != NULL) {
        mbedtls_x509_crt_free(&ssl->own_crt);
    }
    free(ssl->servername);
    free(ssl->hostname);
    free(ssl);
}

/* The socket under mbedtls: nonblocking reads and writes, the error of a
 * failed one kept for SSL_ERROR_SYSCALL (nginx reads errno). */
static int bio_send(void *p, const unsigned char *buf, size_t len) {
    SSL *s = p;
    ssize_t n = write(s->fd, buf, len);
    if (n >= 0) {
        return (int)n;
    }
    if (errno == EAGAIN || errno == EINTR) {
        return MBEDTLS_ERR_SSL_WANT_WRITE;
    }
    s->sys_errno = errno;
    return errno == EPIPE || errno == ECONNRESET ? MBEDTLS_ERR_NET_CONN_RESET
                                                 : MBEDTLS_ERR_NET_SEND_FAILED;
}

static int bio_recv(void *p, unsigned char *buf, size_t len) {
    SSL *s = p;
    ssize_t n = read(s->fd, buf, len);
    if (n >= 0) {
        return (int)n;
    }
    if (errno == EAGAIN || errno == EINTR) {
        return MBEDTLS_ERR_SSL_WANT_READ;
    }
    s->sys_errno = errno;
    return errno == ECONNRESET ? MBEDTLS_ERR_NET_CONN_RESET : MBEDTLS_ERR_NET_RECV_FAILED;
}

/* The endpoint is known: mbedtls's context, on the context's
 * configuration for it. */
static int ssl_setup(SSL *s, int server) {
    SSL_CTX *ctx = s->initial_ctx;
    int ret;
    if (s->set_up) {
        return 1;
    }
    s->server = server;
    if (ctx->conf[server ? 0 : 1] == NULL && (ctx->conf[server ? 0 : 1] = conf_make(ctx, server)) == NULL) {
        return 0;
    }
    s->conf = ctx->conf[server ? 0 : 1];
    s->conf->refs++;
    ret = mbedtls_ssl_setup(&s->m, &s->conf->conf);
    if (ret != 0) {
        myos_err_mbedtls(ERR_LIB_SSL, 0, ret);
        return 0;
    }
    mbedtls_ssl_set_bio(&s->m, s, bio_send, bio_recv, NULL);
    s->set_up = 1;
    if (s->hostname != NULL) {
        mbedtls_ssl_set_hostname(&s->m, s->hostname);
    }
    return 1;
}

int SSL_set_fd(SSL *s, int fd) {
    s->fd = fd;
    s->bio.fd = fd;
    return 1;
}

void SSL_set_connect_state(SSL *s) {
    (void)ssl_setup(s, 0);
}

void SSL_set_accept_state(SSL *s) {
    (void)ssl_setup(s, 1);
}

int SSL_is_server(const SSL *s) {
    return s->server;
}

int SSL_get_ex_new_index(long argl, void *argp, CRYPTO_EX_new *new_func,
    CRYPTO_EX_dup *dup_func, CRYPTO_EX_free *free_func) {
    (void)argl;
    (void)argp;
    (void)new_func;
    (void)dup_func;
    (void)free_func;
    return myos_ex_index(0);
}

int SSL_set_ex_data(SSL *ssl, int idx, void *data) {
    if (idx < 0 || idx >= MYOS_EX_SLOTS) {
        return 0;
    }
    ssl->ex[idx] = data;
    return 1;
}

void *SSL_get_ex_data(const SSL *ssl, int idx) {
    return idx >= 0 && idx < MYOS_EX_SLOTS ? ssl->ex[idx] : NULL;
}

int SSL_get_ex_data_X509_STORE_CTX_idx(void) {
    return 0;
}

/* An mbedtls failure as OpenSSL's: the SSL_ERROR_* of the connection,
 * and for SSL_ERROR_SSL the reason in the error queue. */
static int is_http(const unsigned char *b) {
    static const char *const methods[] = {"GET ", "POST", "HEAD", "PUT ", "DELE", "OPTI",
        "PATC", "TRAC", NULL};
    for (int i = 0; methods[i] != NULL; i++) {
        if (memcmp(b, methods[i], 4) == 0) {
            return SSL_R_HTTP_REQUEST;
        }
    }
    return memcmp(b, "CONN", 4) == 0 ? SSL_R_HTTPS_PROXY_REQUEST : 0;
}

static int ssl_fail(SSL *s, int ret) {
    int reason = 0;
    switch (ret) {
    case MBEDTLS_ERR_SSL_WANT_READ:
        s->error = SSL_ERROR_WANT_READ;
        return -1;
    case MBEDTLS_ERR_SSL_WANT_WRITE:
        s->error = SSL_ERROR_WANT_WRITE;
        return -1;
    case MBEDTLS_ERR_SSL_PEER_CLOSE_NOTIFY:
        s->shutdown |= SSL_RECEIVED_SHUTDOWN;
        s->error = SSL_ERROR_ZERO_RETURN;
        return 0;
    case MBEDTLS_ERR_SSL_CONN_EOF:
        s->error = SSL_ERROR_SYSCALL;
        errno = 0;
        return 0;
    case MBEDTLS_ERR_NET_CONN_RESET:
    case MBEDTLS_ERR_NET_SEND_FAILED:
    case MBEDTLS_ERR_NET_RECV_FAILED:
        s->error = SSL_ERROR_SYSCALL;
        errno = s->sys_errno;
        return -1;
    case MBEDTLS_ERR_SSL_FATAL_ALERT_MESSAGE:
        reason = SSL_AD_REASON_OFFSET + (s->m.in_msg != NULL ? s->m.in_msg[1] : 0);
        break;
    case MBEDTLS_ERR_SSL_INVALID_RECORD:
        /* Plain HTTP to an HTTPS port: the record's first bytes. */
        if (s->server && s->m.in_hdr != NULL) {
            reason = is_http(s->m.in_hdr);
        }
        if (reason == 0) {
            reason = SSL_R_WRONG_VERSION_NUMBER;
        }
        break;
    case MBEDTLS_ERR_SSL_BAD_PROTOCOL_VERSION:
        reason = SSL_R_UNSUPPORTED_PROTOCOL;
        break;
    case MBEDTLS_ERR_SSL_HANDSHAKE_FAILURE:
        reason = SSL_R_NO_SHARED_CIPHER;
        break;
    case MBEDTLS_ERR_SSL_UNEXPECTED_MESSAGE:
        reason = SSL_R_UNEXPECTED_MESSAGE;
        break;
    case MBEDTLS_ERR_SSL_DECODE_ERROR:
        reason = SSL_R_LENGTH_MISMATCH;
        break;
    case MBEDTLS_ERR_SSL_INVALID_MAC:
        reason = SSL_R_DECRYPTION_FAILED_OR_BAD_RECORD_MAC;
        break;
    case MBEDTLS_ERR_X509_CERT_VERIFY_FAILED:
        reason = SSL_R_CERTIFICATE_VERIFY_FAILED;
        break;
    default:
        break;
    }
    s->error = SSL_ERROR_SSL;
    myos_err_mbedtls(ERR_LIB_SSL, reason, ret);
    return -1;
}

static void info(SSL *s, int where, int ret) {
    if (s->ctx->info_cb != NULL) {
        s->ctx->info_cb(s, where, ret);
    } else if (s->initial_ctx->info_cb != NULL) {
        s->initial_ctx->info_cb(s, where, ret);
    }
}

static void handshake_done(SSL *s) {
    const mbedtls_x509_crt *peer = mbedtls_ssl_get_peer_cert(&s->m);
    s->handshake_done = 1;
    s->handshaking = 0;
    if (s->verify_result == X509_V_OK && peer != NULL) {
        /* The name is nginx's to check (ngx_ssl_check_host). */
        uint32_t flags = mbedtls_ssl_get_verify_result(&s->m) & ~MBEDTLS_X509_BADCERT_CN_MISMATCH;
        s->verify_result = myos_verify_flags_to_x509(flags, peer);
    }
    if (!s->server && s->session != NULL) {
        mbedtls_ssl_session ms;
        mbedtls_ssl_session_init(&ms);
        if (mbedtls_ssl_get_session(&s->m, &ms) == 0) {
            s->reused = mbedtls_ssl_session_get_id_len(&ms) == s->session->id_len
                && memcmp(mbedtls_ssl_session_get_id(&ms), s->session->id, s->session->id_len) == 0;
        }
        mbedtls_ssl_session_free(&ms);
    }
    if (!s->server && (s->initial_ctx->cache_mode & SSL_SESS_CACHE_CLIENT)
        && s->initial_ctx->new_session_cb != NULL) {
        SSL_SESSION *sess = SSL_get1_session(s);
        if (sess != NULL && !s->initial_ctx->new_session_cb(s, sess)) {
            SSL_SESSION_free(sess);
        }
    }
    info(s, SSL_CB_HANDSHAKE_DONE, 1);
}

int SSL_do_handshake(SSL *s) {
    int ret;
    if (s->handshake_done) {
        return 1;
    }
    if (!s->set_up) {
        s->error = SSL_ERROR_SSL;
        return -1;
    }
    if (!s->handshaking) {
        s->handshaking = 1;
        info(s, SSL_CB_HANDSHAKE_START, 1);
    }
    if (s->server) {
        info(s, SSL_CB_ACCEPT_LOOP, 1);
    }
    current = s;
    ret = mbedtls_ssl_handshake(&s->m);
    current = NULL;
    if (ret == 0) {
        handshake_done(s);
        return 1;
    }
    return ssl_fail(s, ret);
}

int SSL_read(SSL *ssl, void *buf, int num) {
    int ret;
    if (!ssl->handshake_done && (ret = SSL_do_handshake(ssl)) <= 0) {
        return ret;
    }
    ret = mbedtls_ssl_read(&ssl->m, buf, (size_t)num);
    if (ret > 0) {
        return ret;
    }
    if (ret == 0) {
        /* The transport closed without a close_notify. */
        ssl->error = SSL_ERROR_SYSCALL;
        errno = 0;
        return 0;
    }
    return ssl_fail(ssl, ret);
}

int SSL_write(SSL *ssl, const void *buf, int num) {
    int ret;
    if (!ssl->handshake_done && (ret = SSL_do_handshake(ssl)) <= 0) {
        return ret;
    }
    ret = mbedtls_ssl_write(&ssl->m, buf, (size_t)num);
    return ret >= 0 ? ret : ssl_fail(ssl, ret);
}

/* Send our close_notify (1 when the peer's is there too or not waited
 * for, else 0), then on a second call wait for the peer's. */
int SSL_shutdown(SSL *s) {
    int ret;
    if (!s->handshake_done) {
        return 1;
    }
    if (s->quiet_shutdown) {
        s->shutdown = SSL_SENT_SHUTDOWN | SSL_RECEIVED_SHUTDOWN;
        return 1;
    }
    if (!(s->shutdown & SSL_SENT_SHUTDOWN)) {
        ret = mbedtls_ssl_close_notify(&s->m);
        if (ret != 0) {
            return ssl_fail(s, ret);
        }
        s->shutdown |= SSL_SENT_SHUTDOWN;
        return s->shutdown & SSL_RECEIVED_SHUTDOWN ? 1 : 0;
    }
    if (s->shutdown & SSL_RECEIVED_SHUTDOWN) {
        return 1;
    }
    {
        unsigned char b[256];
        ret = mbedtls_ssl_read(&s->m, b, sizeof b);
    }
    if (ret == MBEDTLS_ERR_SSL_PEER_CLOSE_NOTIFY || ret >= 0) {
        s->shutdown |= SSL_RECEIVED_SHUTDOWN;
        return 1;
    }
    return ssl_fail(s, ret);
}

int SSL_get_error(const SSL *s, int ret_code) {
    return ret_code > 0 ? SSL_ERROR_NONE : s->error;
}

int SSL_in_init(const SSL *s) {
    return !s->handshake_done;
}

int SSL_is_init_finished(const SSL *s) {
    return s->handshake_done;
}

void SSL_set_quiet_shutdown(SSL *ssl, int mode) {
    ssl->quiet_shutdown = mode;
}

void SSL_set_shutdown(SSL *ssl, int mode) {
    ssl->shutdown = mode;
}

int SSL_get_shutdown(const SSL *ssl) {
    return ssl->shutdown;
}

BIO *SSL_get_rbio(const SSL *s) {
    return (BIO *)&s->bio;
}

BIO *SSL_get_wbio(const SSL *s) {
    return (BIO *)&s->bio;
}

SSL_CTX *SSL_get_SSL_CTX(const SSL *ssl) {
    return ssl->ctx;
}

/* During the SNI callback: the certificates, the trusted ones and the
 * verification of the context nginx chose for the name. */
SSL_CTX *SSL_set_SSL_CTX(SSL *ssl, SSL_CTX *ctx) {
    if (ctx == NULL) {
        ctx = ssl->initial_ctx;
    }
    if (ctx == ssl->ctx) {
        return ctx;
    }
    if (ssl->handshaking && ssl->server) {
        for (int i = 0; i < ctx->ncerts; i++) {
            struct myos_cert *c = &ctx->certs[i];
            if (c->x509 == NULL || c->pkey == NULL || cert_build(c) != 0
                || mbedtls_ssl_set_hs_own_cert(&ssl->m, &c->crts, &c->pkey->pk) != 0) {
                myos_err_put(ERR_LIB_SSL, SSL_R_NO_CERTIFICATE_SET, NULL);
                return NULL;
            }
        }
        if (ctx->store->ncas > 0) {
            mbedtls_ssl_set_hs_ca_chain(&ssl->m, &ctx->store->cas,
                ctx->store->ncrls > 0 ? &ctx->store->crls : NULL);
        }
    }
    SSL_CTX_up_ref(ctx);
    SSL_CTX_free(ssl->ctx);
    ssl->ctx = ctx;
    return ctx;
}

static int sni_cb(void *p, mbedtls_ssl_context *m, const unsigned char *name, size_t len) {
    SSL *s = (SSL *)m;
    int ad = SSL_AD_UNRECOGNIZED_NAME;
    (void)p;
    free(s->servername);
    s->servername = malloc(len + 1);
    if (s->servername == NULL) {
        return -1;
    }
    memcpy(s->servername, name, len);
    s->servername[len] = '\0';
    if (s->initial_ctx->servername_cb == NULL) {
        return 0;
    }
    return s->initial_ctx->servername_cb(s, &ad, NULL) == SSL_TLSEXT_ERR_ALERT_FATAL ? -1 : 0;
}

unsigned long SSL_set_options(SSL *s, unsigned long op) {
    return s->options |= op;
}

unsigned long SSL_clear_options(SSL *s, unsigned long op) {
    return s->options &= ~op;
}

unsigned long SSL_get_options(const SSL *s) {
    return s->options;
}

void SSL_set_verify(SSL *s, int mode, SSL_verify_cb callback) {
    s->verify_mode = mode;
    s->verify_cb = callback;
    if (s->set_up && !s->handshake_done) {
        mbedtls_ssl_set_hs_authmode(&s->m, authmode(mode, callback));
    }
}

void SSL_set_verify_depth(SSL *s, int depth) {
    s->verify_depth = depth;
}

long SSL_get_verify_result(const SSL *ssl) {
    return ssl->verify_result;
}

X509 *SSL_get_peer_certificate(const SSL *s) {
    const mbedtls_x509_crt *peer = mbedtls_ssl_get_peer_cert(&s->m);
    return peer != NULL ? myos_x509_from_der(peer->raw.p, peer->raw.len) : NULL;
}

X509 *SSL_get_certificate(const SSL *ssl) {
    return ssl->own_x509 != NULL ? ssl->own_x509 : ssl->ctx->certs[0].x509;
}

/* A connection's own certificate (variables in ssl_certificate, which
 * nginx only does in a certificate callback, not here): not taken. */
int SSL_use_certificate(SSL *ssl, X509 *x) {
    (void)ssl;
    (void)x;
    myos_err_put(ERR_LIB_SSL, 0, "per-connection certificates are not supported");
    return 0;
}

int SSL_use_PrivateKey(SSL *ssl, EVP_PKEY *pkey) {
    (void)ssl;
    (void)pkey;
    myos_err_put(ERR_LIB_SSL, 0, "per-connection certificates are not supported");
    return 0;
}

const char *SSL_get_servername(const SSL *s, const int type) {
    (void)type;
    return s->server ? s->servername : s->hostname;
}

int SSL_set_tlsext_host_name(SSL *s, const char *name) {
    free(s->hostname);
    s->hostname = strdup(name);
    if (s->hostname == NULL) {
        return 0;
    }
    return !s->set_up || mbedtls_ssl_set_hostname(&s->m, name) == 0;
}

const char *SSL_get_version(const SSL *s) {
    return s->handshake_done ? mbedtls_ssl_get_version(&s->m) : "unknown";
}

int SSL_version(const SSL *s) {
    (void)s;
    return TLS1_2_VERSION;
}

STACK_OF(SSL_CIPHER) *SSL_get_ciphers(const SSL *s) {
    (void)s;
    return NULL;
}

SSL_SESSION *SSL_get1_session(SSL *ssl) {
    mbedtls_ssl_session ms;
    SSL_SESSION *s = NULL;
    if (!ssl->handshake_done) {
        return NULL;
    }
    mbedtls_ssl_session_init(&ms);
    if (mbedtls_ssl_get_session(&ssl->m, &ms) == 0) {
        s = session_from(&ms, ssl->initial_ctx);
    }
    mbedtls_ssl_session_free(&ms);
    return s;
}

SSL_SESSION *SSL_get0_session(const SSL *s) {
    SSL *m = (SSL *)s;
    if (m->session == NULL && m->handshake_done) {
        m->session = SSL_get1_session(m);
    }
    return m->session;
}

/* A client's session to resume (before the handshake). */
int SSL_set_session(SSL *to, SSL_SESSION *session) {
    mbedtls_ssl_session ms;
    int ret;
    if (!to->set_up || to->server) {
        return 0;
    }
    mbedtls_ssl_session_init(&ms);
    ret = mbedtls_ssl_session_load(&ms, session->data, session->data_len);
    if (ret == 0) {
        ret = mbedtls_ssl_set_session(&to->m, &ms);
    }
    mbedtls_ssl_session_free(&ms);
    if (ret != 0) {
        myos_err_mbedtls(ERR_LIB_SSL, 0, ret);
        return 0;
    }
    SSL_SESSION_up_ref(session);
    SSL_SESSION_free(to->session);
    to->session = session;
    return 1;
}

int SSL_session_reused(const SSL *s) {
    return s->reused;
}
