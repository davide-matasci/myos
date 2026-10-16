/* myos TLS client over LibreSSL's libtls: a handshake on a connected /net
 * TCP data fd, reads and writes, the peer verified against the Mozilla CA
 * bundle curl's port ships as /lib/cacert.pem. The Rust side (lib.rs) holds
 * the connection and does the fd I/O. */
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <tls.h>

/* Rust syscall wrappers: 0 means nothing yet (the /net data file's empty
 * read), -1 an error. */
extern int myos_tls_fd_read(int fd, unsigned char *buf, size_t len);
extern int myos_tls_fd_write(int fd, const unsigned char *buf, size_t len);

/* /net TCP reads return 0 when the rx queue is empty. Giving libtls a
 * poll-again answer at once makes the handshake busy-loop in userspace
 * and starves netd on a single core: poll with real read/write syscalls
 * (as user/dns DATA_POLLS does) so the scheduler can run netd. Recv must
 * wait for netd to TX the ClientHello and pump the ServerHello and the
 * certificates into DATA: chains need headroom under load. */
enum { BIO_POLLS = 800000 };
/* libtls poll-again answers the handshake takes before giving up. */
enum { HANDSHAKE_ROUNDS = 8 };

typedef struct {
    struct tls *ctx;
    int fd;
    int ready;
    char err[160];
} myos_tls_conn;

static ssize_t cb_read(struct tls *ctx, void *buf, size_t len, void *arg) {
    myos_tls_conn *c = arg;
    (void)ctx;
    for (int i = 0; i < BIO_POLLS; i++) {
        int n = myos_tls_fd_read(c->fd, buf, len);
        if (n < 0) {
            return -1;
        }
        if (n > 0) {
            return n;
        }
    }
    return TLS_WANT_POLLIN;
}

static ssize_t cb_write(struct tls *ctx, const void *buf, size_t len, void *arg) {
    myos_tls_conn *c = arg;
    (void)ctx;
    for (int i = 0; i < BIO_POLLS; i++) {
        int n = myos_tls_fd_write(c->fd, buf, len);
        if (n < 0) {
            return -1;
        }
        if (n > 0) {
            return n;
        }
    }
    return TLS_WANT_POLLOUT;
}

static void keep_error(myos_tls_conn *c, const char *what) {
    const char *e = c->ctx ? tls_error(c->ctx) : NULL;
    if (e == NULL || e[0] == '\0') {
        e = what;
    }
    strncpy(c->err, e, sizeof(c->err) - 1);
    c->err[sizeof(c->err) - 1] = '\0';
}

/* A verified TLS 1.2 or 1.3 connection on `fd`, to the server named
 * `sni_host` (sent as SNI, checked against the certificate); NULL when
 * the handshake fails, with the reason from myos_tls_error. */
myos_tls_conn *myos_tls_handshake(int fd, const char *sni_host) {
    static int inited;
    myos_tls_conn *c = calloc(1, sizeof(*c));
    if (c == NULL) {
        return NULL;
    }
    c->fd = fd;
    if (!inited) {
        if (tls_init() != 0) {
            strcpy(c->err, "tls_init failed");
            return c;
        }
        inited = 1;
    }
    struct tls_config *cfg = tls_config_new();
    if (cfg == NULL) {
        strcpy(c->err, "tls_config_new failed");
        return c;
    }
    if (tls_config_set_ca_file(cfg, "/lib/cacert.pem") != 0) {
        strncpy(c->err, tls_config_error(cfg), sizeof(c->err) - 1);
        tls_config_free(cfg);
        return c;
    }
    c->ctx = tls_client();
    if (c->ctx == NULL) {
        strcpy(c->err, "tls_client failed");
        tls_config_free(cfg);
        return c;
    }
    int rc = tls_configure(c->ctx, cfg);
    tls_config_free(cfg);
    if (rc != 0) {
        keep_error(c, "tls_configure failed");
        return c;
    }
    if (tls_connect_cbs(c->ctx, cb_read, cb_write, c, sni_host) != 0) {
        keep_error(c, "tls_connect failed");
        return c;
    }
    int rounds = 0;
    while ((rc = tls_handshake(c->ctx)) != 0) {
        if (rc != TLS_WANT_POLLIN && rc != TLS_WANT_POLLOUT) {
            keep_error(c, "handshake failed");
            return c;
        }
        if (++rounds > HANDSHAKE_ROUNDS) {
            strcpy(c->err, "handshake timed out");
            return c;
        }
    }
    c->ready = 1;
    return c;
}

int myos_tls_ready(const myos_tls_conn *c) {
    return c != NULL && c->ready;
}

const char *myos_tls_error(const myos_tls_conn *c) {
    return c ? c->err : "no connection";
}

/* `len` bytes written, all of them: how many, or -1. */
int myos_tls_write(myos_tls_conn *c, const unsigned char *buf, size_t len) {
    if (!c || !c->ready) {
        return -1;
    }
    size_t done = 0;
    while (done < len) {
        ssize_t n = tls_write(c->ctx, buf + done, len - done);
        if (n == TLS_WANT_POLLIN || n == TLS_WANT_POLLOUT) {
            continue;
        }
        if (n < 0) {
            keep_error(c, "write failed");
            return -1;
        }
        done += (size_t)n;
    }
    return (int)done;
}

/* Up to `len` bytes: how many, 0 when nothing came yet or the peer closed,
 * -1 on an error. */
int myos_tls_read(myos_tls_conn *c, unsigned char *buf, size_t len) {
    if (!c || !c->ready) {
        return -1;
    }
    ssize_t n = tls_read(c->ctx, buf, len);
    if (n == TLS_WANT_POLLIN || n == TLS_WANT_POLLOUT) {
        return 0;
    }
    if (n < 0) {
        keep_error(c, "read failed");
        return -1;
    }
    return (int)n;
}

/* close_notify when the connection is up, then everything freed. */
void myos_tls_close(myos_tls_conn *c) {
    if (c == NULL) {
        return;
    }
    if (c->ctx != NULL) {
        if (c->ready) {
            (void)tls_close(c->ctx);
        }
        tls_free(c->ctx);
    }
    free(c);
}
