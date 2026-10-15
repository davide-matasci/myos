/* The OpenSSL API over mbedtls (myos_openssl.h): certificates, names,
 * CRLs, the trust store, PEM, ASN.1 times and integers. */
#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>

#include <mbedtls/asn1.h>
#include <mbedtls/base64.h>
#include <mbedtls/error.h>
#include <mbedtls/oid.h>
#include <mbedtls/pem.h>

#include "myos_ssl_int.h"

/* ---- names ---------------------------------------------------------- */

/* Name ::= SEQUENCE OF SET OF SEQUENCE { type OID, value ANY }. */
static X509_NAME *name_from_der(const unsigned char *der, size_t len) {
    X509_NAME *nm = calloc(1, sizeof *nm);
    unsigned char *p, *end, *set_end, *atv_end;
    size_t l;
    if (nm == NULL || (nm->der = malloc(len ? len : 1)) == NULL) {
        free(nm);
        return NULL;
    }
    memcpy(nm->der, der, len);
    nm->der_len = len;
    p = nm->der;
    end = p + len;
    if (mbedtls_asn1_get_tag(&p, end, &l, MBEDTLS_ASN1_CONSTRUCTED | MBEDTLS_ASN1_SEQUENCE) != 0) {
        return nm;
    }
    end = p + l;
    while (p < end && nm->n < MYOS_NAME_MAX) {
        if (mbedtls_asn1_get_tag(&p, end, &l, MBEDTLS_ASN1_CONSTRUCTED | MBEDTLS_ASN1_SET) != 0) {
            break;
        }
        set_end = p + l;
        while (p < set_end && nm->n < MYOS_NAME_MAX) {
            struct myos_name_attr *a = &nm->attr[nm->n];
            if (mbedtls_asn1_get_tag(&p, set_end, &l,
                    MBEDTLS_ASN1_CONSTRUCTED | MBEDTLS_ASN1_SEQUENCE) != 0) {
                return nm;
            }
            atv_end = p + l;
            if (mbedtls_asn1_get_tag(&p, atv_end, &l, MBEDTLS_ASN1_OID) != 0) {
                return nm;
            }
            a->oid = p;
            a->oid_len = l;
            p += l;
            if (p >= atv_end) {
                return nm;
            }
            a->tag = *p++;
            if (mbedtls_asn1_get_len(&p, atv_end, &l) != 0) {
                return nm;
            }
            a->value.type = a->tag;
            a->value.data = p;
            a->value.length = (int)l;
            nm->entry[nm->n].value = a->value;
            nm->n++;
            p = atv_end;
        }
        p = set_end;
    }
    return nm;
}

/* An attribute's short name (CN, O, ...), or its OID in dotted form. */
static const char *attr_name(const struct myos_name_attr *a, char *buf, size_t size) {
    mbedtls_asn1_buf oid = {MBEDTLS_ASN1_OID, a->oid_len, (unsigned char *)a->oid};
    const char *sn;
    if (mbedtls_oid_get_attr_short_name(&oid, &sn) == 0) {
        return sn;
    }
    if (mbedtls_oid_get_numeric_string(buf, size, &oid) < 0) {
        return "?";
    }
    return buf;
}

X509_NAME *X509_NAME_dup(const X509_NAME *xn) {
    return name_from_der(xn->der, xn->der_len);
}

void X509_NAME_free(X509_NAME *a) {
    if (a != NULL) {
        free(a->der);
        free(a);
    }
}

int X509_NAME_cmp(const X509_NAME *a, const X509_NAME *b) {
    if (a->der_len != b->der_len) {
        return a->der_len < b->der_len ? -1 : 1;
    }
    return memcmp(a->der, b->der, a->der_len);
}

/* An attribute's value as text: printable bytes as they are, others as
 * \XX (OpenSSL's escaping of them). */
static int put_value(BIO *b, const ASN1_STRING *v, const char *special) {
    for (int i = 0; i < v->length; i++) {
        unsigned char c = v->data[i];
        char esc[5];
        if (c < 0x20 || c >= 0x7f) {
            snprintf(esc, sizeof esc, "\\%02X", c);
            if (!myos_bio_puts(b, esc)) {
                return 0;
            }
            continue;
        }
        if (strchr(special, c) != NULL && !myos_bio_append(b, "\\", 1)) {
            return 0;
        }
        if (!myos_bio_append(b, &c, 1)) {
            return 0;
        }
    }
    return 1;
}

/* "/C=US/O=Org/CN=name", the subject in order. */
char *X509_NAME_oneline(const X509_NAME *a, char *buf, int size) {
    BIO *b = myos_bio_mem();
    char oid[64];
    char *out;
    if (b == NULL) {
        return NULL;
    }
    for (int i = 0; i < a->n; i++) {
        myos_bio_puts(b, "/");
        myos_bio_puts(b, attr_name(&a->attr[i], oid, sizeof oid));
        myos_bio_puts(b, "=");
        put_value(b, &a->attr[i].value, "");
    }
    if (buf == NULL) {
        out = malloc(b->len + 1);
        if (out != NULL) {
            memcpy(out, b->buf, b->len + 1);
        }
    } else {
        out = buf;
        if (size > 0) {
            size_t n = b->len < (size_t)size - 1 ? b->len : (size_t)size - 1;
            memcpy(buf, b->buf, n);
            buf[n] = '\0';
        }
    }
    BIO_free(b);
    return out;
}

/* RFC 2253: "CN=name,O=Org,C=US", the last attribute first. */
int X509_NAME_print_ex(BIO *out, const X509_NAME *nm, int indent, unsigned long flags) {
    char oid[64];
    (void)indent;
    (void)flags;
    for (int i = nm->n - 1; i >= 0; i--) {
        if (i != nm->n - 1) {
            myos_bio_puts(out, ",");
        }
        myos_bio_puts(out, attr_name(&nm->attr[i], oid, sizeof oid));
        myos_bio_puts(out, "=");
        if (!put_value(out, &nm->attr[i].value, ",+\"\\<>;")) {
            return -1;
        }
    }
    return 1;
}

int X509_NAME_digest(const X509_NAME *data, const EVP_MD *type, unsigned char *md,
    unsigned int *len) {
    const mbedtls_md_info_t *info = mbedtls_md_info_from_type(type->type);
    if (mbedtls_md(info, data->der, data->der_len, md) != 0) {
        return 0;
    }
    if (len != NULL) {
        *len = mbedtls_md_get_size(info);
    }
    return 1;
}

int X509_NAME_get_index_by_NID(const X509_NAME *name, int nid, int lastpos) {
    if (nid != NID_commonName) {
        return -1;
    }
    for (int i = lastpos + 1; i < name->n; i++) {
        if (name->attr[i].oid_len == MBEDTLS_OID_SIZE(MBEDTLS_OID_AT_CN)
            && memcmp(name->attr[i].oid, MBEDTLS_OID_AT_CN, name->attr[i].oid_len) == 0) {
            return i;
        }
    }
    return -1;
}

X509_NAME_ENTRY *X509_NAME_get_entry(const X509_NAME *name, int loc) {
    return loc >= 0 && loc < name->n ? (X509_NAME_ENTRY *)&name->entry[loc] : NULL;
}

ASN1_STRING *X509_NAME_ENTRY_get_data(const X509_NAME_ENTRY *ne) {
    return (ASN1_STRING *)&ne->value;
}

/* ---- ASN.1 ---------------------------------------------------------- */

const unsigned char *ASN1_STRING_get0_data(const ASN1_STRING *x) {
    return x->data;
}

int ASN1_STRING_length(const ASN1_STRING *x) {
    return x->length;
}

/* "Jan  2 03:04:05 2026 GMT", OpenSSL's form. */
int ASN1_TIME_print(BIO *fp, const ASN1_TIME *a) {
    static const char *const mon[] = {"Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"};
    const mbedtls_x509_time *t = &a->time;
    char buf[40];
    if (t->mon < 1 || t->mon > 12) {
        return 0;
    }
    snprintf(buf, sizeof buf, "%s %2d %02d:%02d:%02d %d GMT", mon[t->mon - 1], t->day,
        t->hour, t->min, t->sec, t->year);
    return myos_bio_puts(fp, buf);
}

time_t myos_asn1_time_t(const ASN1_TIME *a) {
    struct tm tm;
    memset(&tm, 0, sizeof tm);
    tm.tm_year = a->time.year - 1900;
    tm.tm_mon = a->time.mon - 1;
    tm.tm_mday = a->time.day;
    tm.tm_hour = a->time.hour;
    tm.tm_min = a->time.min;
    tm.tm_sec = a->time.sec;
    return mktime(&tm);
}

/* The serial number in hex, two digits a byte ("00" for zero). */
int i2a_ASN1_INTEGER(BIO *bp, const ASN1_INTEGER *a) {
    char hex[3];
    int i = 0;
    if (a->length == 0) {
        return myos_bio_puts(bp, "00") ? 2 : -1;
    }
    /* A leading 0 byte only keeps a positive number positive. */
    if (a->length > 1 && a->data[0] == 0) {
        i = 1;
    }
    for (int n = i; n < a->length; n++) {
        snprintf(hex, sizeof hex, "%02X", a->data[n]);
        if (!myos_bio_puts(bp, hex)) {
            return -1;
        }
    }
    return (a->length - i) * 2;
}

const char *OBJ_nid2sn(int n) {
    switch (n) {
    case NID_commonName: return "CN";
    case NID_X9_62_prime256v1: return "prime256v1";
    case NID_secp384r1: return "secp384r1";
    default: return "UNDEF";
    }
}

int OBJ_sn2nid(const char *s) {
    if (strcmp(s, "prime256v1") == 0) {
        return NID_X9_62_prime256v1;
    }
    if (strcmp(s, "secp384r1") == 0) {
        return NID_secp384r1;
    }
    return NID_undef;
}

/* ---- certificates --------------------------------------------------- */

X509 *myos_x509_from_der(const unsigned char *der, size_t len) {
    X509 *x = calloc(1, sizeof *x);
    int ret;
    if (x == NULL) {
        return NULL;
    }
    x->refs = 1;
    mbedtls_x509_crt_init(&x->crt);
    ret = mbedtls_x509_crt_parse_der(&x->crt, der, len);
    if (ret != 0) {
        myos_err_mbedtls(ERR_LIB_X509, 0, ret);
        mbedtls_x509_crt_free(&x->crt);
        free(x);
        return NULL;
    }
    x->subject = name_from_der(x->crt.subject_raw.p, x->crt.subject_raw.len);
    x->issuer = name_from_der(x->crt.issuer_raw.p, x->crt.issuer_raw.len);
    x->serial.data = x->crt.serial.p;
    x->serial.length = (int)x->crt.serial.len;
    x->not_before.time = x->crt.valid_from;
    x->not_after.time = x->crt.valid_to;
    if (x->subject == NULL || x->issuer == NULL) {
        X509_free(x);
        return NULL;
    }
    return x;
}

void X509_free(X509 *a) {
    if (a != NULL && --a->refs == 0) {
        X509_NAME_free(a->subject);
        X509_NAME_free(a->issuer);
        mbedtls_x509_crt_free(&a->crt);
        free(a);
    }
}

int X509_up_ref(X509 *a) {
    a->refs++;
    return 1;
}

X509_NAME *X509_get_subject_name(const X509 *a) {
    return a->subject;
}

X509_NAME *X509_get_issuer_name(const X509 *a) {
    return a->issuer;
}

ASN1_INTEGER *X509_get_serialNumber(X509 *x) {
    return &x->serial;
}

const ASN1_TIME *X509_get0_notBefore(const X509 *x) {
    return &x->not_before;
}

const ASN1_TIME *X509_get0_notAfter(const X509 *x) {
    return &x->not_after;
}

int X509_digest(const X509 *data, const EVP_MD *type, unsigned char *md, unsigned int *len) {
    const mbedtls_md_info_t *info = mbedtls_md_info_from_type(type->type);
    if (mbedtls_md(info, data->crt.raw.p, data->crt.raw.len, md) != 0) {
        return 0;
    }
    if (len != NULL) {
        *len = mbedtls_md_get_size(info);
    }
    return 1;
}

int X509_get_ex_new_index(long argl, void *argp, CRYPTO_EX_new *new_func,
    CRYPTO_EX_dup *dup_func, CRYPTO_EX_free *free_func) {
    (void)argl;
    (void)argp;
    (void)new_func;
    (void)dup_func;
    (void)free_func;
    return myos_ex_index(2);
}

int X509_set_ex_data(X509 *r, int idx, void *arg) {
    if (idx < 0 || idx >= MYOS_EX_SLOTS) {
        return 0;
    }
    r->ex[idx] = arg;
    return 1;
}

void *X509_get_ex_data(const X509 *r, int idx) {
    return idx >= 0 && idx < MYOS_EX_SLOTS ? r->ex[idx] : NULL;
}

/* A host name against a certificate's name: case-insensitive, a "*" as
 * the whole leftmost label matches one label (not in "*.com"-like names
 * of two labels). */
static int host_matches(const char *pat, size_t plen, const char *host, size_t hlen) {
    if (plen == hlen && strncasecmp(pat, host, plen) == 0) {
        return 1;
    }
    if (plen > 2 && pat[0] == '*' && pat[1] == '.') {
        const char *dot = memchr(host, '.', hlen);
        size_t rest;
        if (dot == NULL || dot == host || memchr(pat + 2, '.', plen - 2) == NULL) {
            return 0;
        }
        rest = hlen - (size_t)(dot - host);
        return rest == plen - 1 && strncasecmp(pat + 1, dot, rest) == 0;
    }
    return 0;
}

/* The DNS names of the subject alternative names, else the common names. */
int X509_check_host(X509 *x, const char *chk, size_t chklen, unsigned int flags,
    char **peername) {
    const mbedtls_x509_sequence *cur;
    int have_dns = 0;
    (void)peername;
    if (chklen == 0) {
        chklen = strlen(chk);
    }
    for (cur = &x->crt.subject_alt_names; cur != NULL && cur->buf.p != NULL; cur = cur->next) {
        mbedtls_x509_subject_alternative_name san;
        if (mbedtls_x509_parse_subject_alt_name(&cur->buf, &san) != 0) {
            continue;
        }
        if (san.type == MBEDTLS_X509_SAN_DNS_NAME) {
            have_dns = 1;
            if (host_matches((const char *)san.san.unstructured_name.p,
                    san.san.unstructured_name.len, chk, chklen)) {
                mbedtls_x509_free_subject_alt_name(&san);
                return 1;
            }
        }
        mbedtls_x509_free_subject_alt_name(&san);
    }
    if (have_dns && !(flags & X509_CHECK_FLAG_ALWAYS_CHECK_SUBJECT)) {
        return 0;
    }
    for (int i = X509_NAME_get_index_by_NID(x->subject, NID_commonName, -1); i >= 0;
         i = X509_NAME_get_index_by_NID(x->subject, NID_commonName, i)) {
        const ASN1_STRING *v = &x->subject->attr[i].value;
        if (host_matches((const char *)v->data, (size_t)v->length, chk, chklen)) {
            return 1;
        }
    }
    return 0;
}

int X509_check_issued(X509 *issuer, X509 *subject) {
    return X509_NAME_cmp(issuer->subject, subject->issuer) == 0 ? X509_V_OK
        : X509_V_ERR_UNABLE_TO_GET_ISSUER_CERT;
}

/* mbedtls's verification flags as the X509_V_ERR_* OpenSSL would report. */
long myos_verify_flags_to_x509(uint32_t flags, const mbedtls_x509_crt *crt) {
    if (flags == 0) {
        return X509_V_OK;
    }
    if (flags & MBEDTLS_X509_BADCERT_EXPIRED) {
        return X509_V_ERR_CERT_HAS_EXPIRED;
    }
    if (flags & MBEDTLS_X509_BADCERT_FUTURE) {
        return X509_V_ERR_CERT_NOT_YET_VALID;
    }
    if (flags & MBEDTLS_X509_BADCERT_REVOKED) {
        return X509_V_ERR_CERT_REVOKED;
    }
    if (flags & MBEDTLS_X509_BADCRL_NOT_TRUSTED) {
        return X509_V_ERR_UNABLE_TO_GET_CRL;
    }
    if (flags & MBEDTLS_X509_BADCERT_NOT_TRUSTED) {
        if (crt != NULL && crt->next == NULL && crt->issuer_raw.len == crt->subject_raw.len
            && memcmp(crt->issuer_raw.p, crt->subject_raw.p, crt->issuer_raw.len) == 0) {
            return X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT;
        }
        return X509_V_ERR_UNABLE_TO_GET_ISSUER_CERT_LOCALLY;
    }
    if (flags & MBEDTLS_X509_BADCERT_CN_MISMATCH) {
        return X509_V_ERR_HOSTNAME_MISMATCH;
    }
    if (flags & (MBEDTLS_X509_BADCERT_KEY_USAGE | MBEDTLS_X509_BADCERT_EXT_KEY_USAGE
            | MBEDTLS_X509_BADCERT_NS_CERT_TYPE)) {
        return X509_V_ERR_INVALID_PURPOSE;
    }
    return X509_V_ERR_CERT_REJECTED;
}

const char *X509_verify_cert_error_string(long n) {
    switch (n) {
    case X509_V_OK: return "ok";
    case X509_V_ERR_UNABLE_TO_GET_ISSUER_CERT: return "unable to get issuer certificate";
    case X509_V_ERR_UNABLE_TO_GET_CRL: return "unable to get certificate CRL";
    case X509_V_ERR_CERT_NOT_YET_VALID: return "certificate is not yet valid";
    case X509_V_ERR_CERT_HAS_EXPIRED: return "certificate has expired";
    case X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT: return "self-signed certificate";
    case X509_V_ERR_SELF_SIGNED_CERT_IN_CHAIN: return "self-signed certificate in certificate chain";
    case X509_V_ERR_UNABLE_TO_GET_ISSUER_CERT_LOCALLY: return "unable to get local issuer certificate";
    case X509_V_ERR_CERT_CHAIN_TOO_LONG: return "certificate chain too long";
    case X509_V_ERR_CERT_REVOKED: return "certificate revoked";
    case X509_V_ERR_INVALID_PURPOSE: return "unsupported certificate purpose";
    case X509_V_ERR_CERT_REJECTED: return "certificate rejected";
    case X509_V_ERR_HOSTNAME_MISMATCH: return "hostname mismatch";
    default: return "certificate verify error";
    }
}

/* ---- CRLs and the store --------------------------------------------- */

void X509_CRL_free(X509_CRL *a) {
    if (a != NULL && --a->refs == 0) {
        mbedtls_x509_crl_free(&a->crl);
        free(a);
    }
}

int X509_CRL_up_ref(X509_CRL *a) {
    a->refs++;
    return 1;
}

X509_STORE *X509_STORE_new(void) {
    X509_STORE *s = calloc(1, sizeof *s);
    if (s != NULL) {
        mbedtls_x509_crt_init(&s->cas);
        mbedtls_x509_crl_init(&s->crls);
    }
    return s;
}

void X509_STORE_free(X509_STORE *v) {
    if (v != NULL) {
        mbedtls_x509_crt_free(&v->cas);
        mbedtls_x509_crl_free(&v->crls);
        free(v);
    }
}

int X509_STORE_add_cert(X509_STORE *ctx, X509 *x) {
    int ret;
    for (const mbedtls_x509_crt *c = &ctx->cas; ctx->ncas && c != NULL; c = c->next) {
        if (c->raw.len == x->crt.raw.len && memcmp(c->raw.p, x->crt.raw.p, c->raw.len) == 0) {
            myos_err_put(ERR_LIB_X509, X509_R_CERT_ALREADY_IN_HASH_TABLE, NULL);
            return 0;
        }
    }
    ret = mbedtls_x509_crt_parse_der(&ctx->cas, x->crt.raw.p, x->crt.raw.len);
    if (ret != 0) {
        myos_err_mbedtls(ERR_LIB_X509, 0, ret);
        return 0;
    }
    ctx->ncas++;
    return 1;
}

int X509_STORE_add_crl(X509_STORE *ctx, X509_CRL *x) {
    int ret = mbedtls_x509_crl_parse_der(&ctx->crls, x->crl.raw.p, x->crl.raw.len);
    if (ret != 0) {
        myos_err_mbedtls(ERR_LIB_X509, 0, ret);
        return 0;
    }
    ctx->ncrls++;
    return 1;
}

int X509_STORE_set_flags(X509_STORE *ctx, unsigned long flags) {
    ctx->flags |= flags;
    return 1;
}

/* ---- PEM ------------------------------------------------------------ */

/* The next PEM block of a kind at the BIO's position: its DER (decoded by
 * mbedtls), the position moved past it. PEM_R_NO_START_LINE when there is
 * none, as OpenSSL says at the end of a file of certificates. */
static int pem_next(BIO *bp, const char *const *labels, unsigned char **der, size_t *len,
    const char **label, size_t *block_start, size_t *block_len) {
    char header[64], footer[64];
    const char *best = NULL, *best_label = NULL;
    for (int i = 0; labels[i] != NULL; i++) {
        const char *p;
        snprintf(header, sizeof header, "-----BEGIN %s-----", labels[i]);
        p = strstr((const char *)bp->buf + bp->pos, header);
        if (p != NULL && (best == NULL || p < best)) {
            best = p;
            best_label = labels[i];
        }
    }
    if (best == NULL) {
        myos_err_put(ERR_LIB_PEM, PEM_R_NO_START_LINE, NULL);
        return -1;
    }
    snprintf(header, sizeof header, "-----BEGIN %s-----", best_label);
    snprintf(footer, sizeof footer, "-----END %s-----", best_label);
    *label = best_label;
    *block_start = (size_t)(best - (const char *)bp->buf);
    {
        const char *end = strstr(best, footer);
        if (end == NULL) {
            myos_err_put(ERR_LIB_PEM, 0, "no end line");
            return -1;
        }
        *block_len = (size_t)(end + strlen(footer) - best);
    }
    if (der != NULL) {
        mbedtls_pem_context pem;
        const unsigned char *buf;
        size_t use_len, blen;
        int ret;
        mbedtls_pem_init(&pem);
        ret = mbedtls_pem_read_buffer(&pem, header, footer, (const unsigned char *)best,
            NULL, 0, &use_len);
        if (ret != 0) {
            myos_err_mbedtls(ERR_LIB_PEM, 0, ret);
            mbedtls_pem_free(&pem);
            return -1;
        }
        buf = mbedtls_pem_get_buffer(&pem, &blen);
        *der = malloc(blen ? blen : 1);
        if (*der == NULL) {
            mbedtls_pem_free(&pem);
            return -1;
        }
        memcpy(*der, buf, blen);
        *len = blen;
        mbedtls_pem_free(&pem);
    }
    bp->pos = *block_start + *block_len;
    return 0;
}

static X509 *read_x509(BIO *bp, const char *const *labels) {
    unsigned char *der;
    size_t len, start, blen;
    const char *label;
    X509 *x;
    if (pem_next(bp, labels, &der, &len, &label, &start, &blen) != 0) {
        return NULL;
    }
    x = myos_x509_from_der(der, len);
    free(der);
    return x;
}

X509 *PEM_read_bio_X509(BIO *bp, X509 **x, pem_password_cb *cb, void *u) {
    static const char *const labels[] = {"CERTIFICATE", "X509 CERTIFICATE", NULL};
    (void)x;
    (void)cb;
    (void)u;
    return read_x509(bp, labels);
}

/* A "TRUSTED CERTIFICATE" too: its DER begins with the certificate (the
 * trust settings after it are ignored). */
X509 *PEM_read_bio_X509_AUX(BIO *bp, X509 **x, pem_password_cb *cb, void *u) {
    static const char *const labels[] = {"CERTIFICATE", "X509 CERTIFICATE",
        "TRUSTED CERTIFICATE", NULL};
    (void)x;
    (void)cb;
    (void)u;
    return read_x509(bp, labels);
}

X509_CRL *PEM_read_bio_X509_CRL(BIO *bp, X509_CRL **x, pem_password_cb *cb, void *u) {
    static const char *const labels[] = {"X509 CRL", NULL};
    unsigned char *der;
    size_t len, start, blen;
    const char *label;
    X509_CRL *crl;
    int ret;
    (void)x;
    (void)cb;
    (void)u;
    if (pem_next(bp, labels, &der, &len, &label, &start, &blen) != 0) {
        return NULL;
    }
    crl = calloc(1, sizeof *crl);
    if (crl == NULL) {
        free(der);
        return NULL;
    }
    crl->refs = 1;
    mbedtls_x509_crl_init(&crl->crl);
    ret = mbedtls_x509_crl_parse_der(&crl->crl, der, len);
    free(der);
    if (ret != 0) {
        myos_err_mbedtls(ERR_LIB_X509, 0, ret);
        X509_CRL_free(crl);
        return NULL;
    }
    return crl;
}

/* A key in PEM (PKCS#8, RSA or EC), mbedtls's parser on the block, with
 * the password the callback gives for an encrypted one. */
EVP_PKEY *PEM_read_bio_PrivateKey(BIO *bp, EVP_PKEY **x, pem_password_cb *cb, void *u) {
    static const char *const labels[] = {"PRIVATE KEY", "RSA PRIVATE KEY",
        "EC PRIVATE KEY", "ENCRYPTED PRIVATE KEY", NULL};
    char pwd[1024];
    int pwdlen = 0;
    size_t start, blen;
    const char *label;
    unsigned char *block;
    EVP_PKEY *k;
    int ret;
    (void)x;
    if (pem_next(bp, labels, NULL, NULL, &label, &start, &blen) != 0) {
        return NULL;
    }
    if (cb != NULL) {
        pwdlen = cb(pwd, sizeof pwd, 0, u);
        if (pwdlen < 0) {
            pwdlen = 0;
        }
    }
    block = malloc(blen + 1);
    k = EVP_PKEY_new();
    if (block == NULL || k == NULL) {
        free(block);
        EVP_PKEY_free(k);
        return NULL;
    }
    memcpy(block, bp->buf + start, blen);
    block[blen] = '\0';
    ret = mbedtls_pk_parse_key(&k->pk, block, blen + 1,
        pwdlen ? (const unsigned char *)pwd : NULL, (size_t)pwdlen, myos_rng, NULL);
    mbedtls_platform_zeroize(block, blen);
    mbedtls_platform_zeroize(pwd, sizeof pwd);
    free(block);
    if (ret != 0) {
        myos_err_mbedtls(ERR_LIB_PEM, 0, ret);
        EVP_PKEY_free(k);
        return NULL;
    }
    return k;
}

/* A certificate in PEM, 64 base64 characters a line. */
int PEM_write_bio_X509(BIO *bp, X509 *x) {
    size_t olen;
    unsigned char *b64;
    size_t cap = (x->crt.raw.len + 2) / 3 * 4 + 1;
    b64 = malloc(cap);
    if (b64 == NULL) {
        return 0;
    }
    if (mbedtls_base64_encode(b64, cap, &olen, x->crt.raw.p, x->crt.raw.len) != 0) {
        free(b64);
        return 0;
    }
    myos_bio_puts(bp, "-----BEGIN CERTIFICATE-----\n");
    for (size_t i = 0; i < olen; i += 64) {
        myos_bio_append(bp, b64 + i, olen - i < 64 ? olen - i : 64);
        myos_bio_puts(bp, "\n");
    }
    free(b64);
    return myos_bio_puts(bp, "-----END CERTIFICATE-----\n");
}
