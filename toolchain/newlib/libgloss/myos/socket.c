/*
 * Userspace BSD sockets over Plan 9 /net + netd (smoltcp).
 * No socket() syscall — outbound TCP (and UDP for DNS) via clone/ctl/data.
 */
#include <errno.h>
#include <fcntl.h>
#include "myos_fmt.h"
#include <poll.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <unistd.h>

#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>

#include "myos_syscalls.h"

#define MYOS_MAX_SOCKS 16
#define CONNECT_TIMEOUT_MS 30000

enum {
    SOCK_UNUSED = 0,
    SOCK_OPEN,
    SOCK_CONNECTING, /* nonblock connect in flight; wait for Established */
    SOCK_CONNECTED,
    SOCK_LISTENING,  /* announced (bound + listening); accept() incoming */
};

struct myos_sock {
    int used;
    int state;
    int type;       /* SOCK_STREAM / SOCK_DGRAM */
    int data_fd;    /* returned to the app; /net/.../data */
    int ctl_fd;     /* kept open through connect */
    int nonblock;   /* O_NONBLOCK via fcntl F_SETFL (userspace-tracked) */
    int so_error;   /* pending SO_ERROR (connect failure); cleared on read */
    unsigned short conv;
    char proto_path[16]; /* "/net/tcp" or "/net/udp" */
    struct sockaddr_in peer;
    int peer_set;
    unsigned short bind_port; /* listener port after bind() */
    int accept_armed;  /* listener: "accept" ctl written, waiting for status */
    int last_accept_seq; /* listener: seq of the last accepted handoff */
};

static struct myos_sock socks[MYOS_MAX_SOCKS];

static struct myos_sock *sock_by_fd(int fd) {
    int i;
    if (fd < 0) {
        return NULL;
    }
    for (i = 0; i < MYOS_MAX_SOCKS; i++) {
        if (socks[i].used && socks[i].data_fd == fd) {
            return &socks[i];
        }
    }
    return NULL;
}

static struct myos_sock *sock_alloc(void) {
    int i;
    for (i = 0; i < MYOS_MAX_SOCKS; i++) {
        if (!socks[i].used) {
            memset(&socks[i], 0, sizeof(socks[i]));
            socks[i].used = 1;
            socks[i].data_fd = -1;
            socks[i].ctl_fd = -1;
            socks[i].last_accept_seq = -1;
            return &socks[i];
        }
    }
    return NULL;
}

static void sock_free(struct myos_sock *s) {
    if (s == NULL) {
        return;
    }
    memset(s, 0, sizeof(*s));
    s->data_fd = -1;
    s->ctl_fd = -1;
}

static int conv_path(char *out, size_t cap, const char *proto_path,
    unsigned short id, const char *leaf) {
    size_t pos = 0;
    int n;
    n = myos_cpy(out + pos, cap - pos, proto_path);
    if (n < 0) return -1;
    pos += (size_t)n;
    if (pos + 1 >= cap) return -1;
    out[pos++] = '/';
    n = myos_u16_dec(out + pos, cap - pos, id);
    if (n < 0) return -1;
    pos += (size_t)n;
    if (pos + 1 >= cap) return -1;
    out[pos++] = '/';
    n = myos_cpy(out + pos, cap - pos, leaf);
    if (n < 0) return -1;
    pos += (size_t)n;
    return (int)pos;
}

static int buf_has(const char *hay, size_t n, const char *needle) {
    size_t m = strlen(needle);
    size_t i;
    if (m == 0 || n < m) {
        return 0;
    }
    for (i = 0; i + m <= n; i++) {
        if (memcmp(hay + i, needle, m) == 0) {
            return 1;
        }
    }
    return 0;
}

static int parse_clone_id(const char *buf, size_t n, unsigned short *out) {
    unsigned short id = 0;
    int any = 0;
    size_t i;
    for (i = 0; i < n; i++) {
        char b = buf[i];
        if (b == '\n' || b == '\r' || b == ' ') {
            break;
        }
        if (b < '0' || b > '9') {
            return -1;
        }
        any = 1;
        if (id > 6553 || (id == 6553 && (b - '0') > 5)) {
            return -1;
        }
        id = (unsigned short)(id * 10 + (b - '0'));
    }
    if (!any) {
        return -1;
    }
    *out = id;
    return 0;
}

static long elapsed_ms(const struct timeval *start) {
    struct timeval now;
    if (gettimeofday(&now, NULL) != 0) {
        return 0;
    }
    return (now.tv_sec - start->tv_sec) * 1000L
        + (now.tv_usec - start->tv_usec) / 1000L;
}

/* Read /net/.../status for an in-flight connect.
 * Returns 1=Established ("connected"), -1=error/hangup (errno set), 0=still waiting.
 * Note: "connecting" must not match "connected" (memcmp length check in buf_has). */
static int connect_status(struct myos_sock *s) {
    char path[64];
    char sbuf[64];
    int st;
    ssize_t nr;
    if (conv_path(path, sizeof path, s->proto_path, s->conv, "status") < 0) {
        errno = EIO;
        return -1;
    }
    st = open(path, O_RDONLY);
    if (st < 0) {
        return 0;
    }
    nr = read(st, sbuf, sizeof sbuf);
    close(st);
    if (nr <= 0) {
        return 0;
    }
    if (buf_has(sbuf, (size_t)nr, "connected")) {
        return 1;
    }
    if (buf_has(sbuf, (size_t)nr, "error")
            || buf_has(sbuf, (size_t)nr, "hangup")) {
        errno = ECONNREFUSED;
        return -1;
    }
    return 0;
}

/* Complete a successful handshake: drop ctl, mark peer, SOCK_CONNECTED. */
static void finish_connect(struct myos_sock *s) {
    if (s->ctl_fd >= 0) {
        close(s->ctl_fd);
        s->ctl_fd = -1;
    }
    s->peer_set = 1;
    s->so_error = 0;
    s->state = SOCK_CONNECTED;
}

static int wait_connected(struct myos_sock *s) {
    struct timeval start;
    if (gettimeofday(&start, NULL) != 0) {
        errno = EIO;
        return -1;
    }
    for (;;) {
        int st = connect_status(s);
        if (st > 0) {
            return 0;
        }
        if (st < 0) {
            return -1;
        }
        if (elapsed_ms(&start) >= CONNECT_TIMEOUT_MS) {
            errno = ETIMEDOUT;
            return -1;
        }
    }
}

/* True EOF vs empty: /net data returns 0 when the smoltcp RX queue is empty.
 * curl/mbedtls treat read/recv 0 as peer close (SSL EOF). */
static int status_is_hangup(struct myos_sock *s) {
    char path[64];
    char sbuf[64];
    int st;
    ssize_t nr;
    if (conv_path(path, sizeof path, s->proto_path, s->conv, "status") < 0) {
        return 0;
    }
    st = open(path, O_RDONLY);
    if (st < 0) {
        return 0;
    }
    nr = read(st, sbuf, sizeof sbuf);
    close(st);
    if (nr <= 0) {
        return 0;
    }
    return buf_has(sbuf, (size_t)nr, "hangup")
        || buf_has(sbuf, (size_t)nr, "error");
}

/* netfs reports RX queue length as st_size on /net/.../data. */
static int data_pending(struct myos_sock *s) {
    char path[64];
    struct stat st;
    if (conv_path(path, sizeof path, s->proto_path, s->conv, "data") < 0) {
        return 0;
    }
    if (stat(path, &st) < 0) {
        return 0;
    }
    return st.st_size > 0;
}

/* Block until RX data, hangup, or (timeout_ms>=0) deadline.
 * Returns 0=data ready, 1=hangup, -1=timeout (errno ETIMEDOUT). */
static int wait_readable(struct myos_sock *s, int timeout_ms) {
    struct timeval start;
    if (gettimeofday(&start, NULL) != 0) {
        /* Fall through to untimed spin if clock missing. */
        timeout_ms = -1;
    }
    for (;;) {
        /* Drain remaining RX before treating hangup as EOF (BSD half-close). */
        if (data_pending(s)) {
            return 0;
        }
        if (status_is_hangup(s)) {
            return 1;
        }
        if (timeout_ms >= 0 && elapsed_ms(&start) >= timeout_ms) {
            errno = ETIMEDOUT;
            return -1;
        }
    }
}

/*
 * Called from _read when the kernel returned 0 bytes on a tracked socket fd.
 * Returns:
 *   0 = not our socket (keep 0)
 *   1 = empty + nonblocking -> EAGAIN
 *   2 = hangup/EOF
 *   3 = blocking wait done, data should be available -> retry read
 */
int myos_socket_empty_read(int fd) {
    struct myos_sock *s = sock_by_fd(fd);
    int wr;
    if (s == NULL || s->state != SOCK_CONNECTED) {
        return 0;
    }
    /* Prefer pending RX over hangup (BSD half-close). _read returned 0, but
     * REP_DATA may have landed after the syscall; never EOF while st_size > 0. */
    if (data_pending(s)) {
        return 3;
    }
    if (status_is_hangup(s)) {
        return 2;
    }
    if (s->nonblock) {
        return 1;
    }
    /* BSD: empty read on a blocking TCP socket waits for data or hangup. */
    wr = wait_readable(s, -1);
    if (wr == 1) {
        /* Re-check RX: hangup can race with a late REP_DATA. */
        if (data_pending(s)) {
            return 3;
        }
        return 2;
    }
    return 3;
}

/* fcntl F_GETFL / F_SETFL for tracked sockets. Returns -1 if not a socket. */
int myos_socket_fcntl(int fd, int cmd, int arg) {
    struct myos_sock *s = sock_by_fd(fd);
    if (s == NULL) {
        return -1;
    }
    if (cmd == F_GETFL) {
        return O_RDWR | (s->nonblock ? O_NONBLOCK : 0);
    }
    if (cmd == F_SETFL) {
        s->nonblock = (arg & O_NONBLOCK) ? 1 : 0;
        return 0;
    }
    errno = EINVAL;
    return -2;
}

/*
 * poll readiness for a tracked socket.
 * Returns: 1 = filled *revents (ready or hangup), 0 = not ready, -1 = not socket.
 */
static int listener_ctl(struct myos_sock *s, const char *cmd);
static int listener_status(struct myos_sock *s, char *out, size_t cap);
/* Last whitespace-separated decimal in an "accepted ..." status = the
 * per-listener handoff seq; -1 when absent. */
static int status_accept_seq(const char *status) {
    int L = (int)strlen(status);
    int j = L - 1;
    const char *t;
    while (j >= 0 && status[j] != ' ') {
        j--;
    }
    if (j < 0) {
        return -1;
    }
    t = status + j + 1;
    if (*t < '0' || *t > '9') {
        return -1;
    }
    {
        unsigned int v = 0;
        while (*t >= '0' && *t <= '9') {
            v = v * 10 + (unsigned)(*t - '0');
            t++;
        }
        if (*t != '\0') {
            return -1;
        }
        return (int)v;
    }
}

int myos_socket_poll(int fd, short events, short *revents) {
    struct myos_sock *s = sock_by_fd(fd);
    short rev = 0;
    int want_in;
    int want_out;
    if (s == NULL) {
        return -1;
    }
    if (revents == NULL) {
        errno = EINVAL;
        return -2;
    }
    want_in = events & (POLLIN | POLLPRI | POLLRDNORM);
    want_out = events & (POLLOUT | POLLWRNORM);

    /* Listening sockets: arm the netd "accept" once (select-driven servers
     * like dropbear select() before accept()), then report POLLIN when the
     * listener status shows "accepted <N>". Without this, select() never
     * wakes for listeners and the server never accepts. */
    if (s->state == SOCK_LISTENING) {
        if (want_in) {
            char stbuf[80];
            if (!s->accept_armed) {
                if (listener_ctl(s, "accept") == 0) {
                    s->accept_armed = 1;
                }
                /* Don't trust the status this call: netd clears any stale
                 * "accepted <old>" when it parks the fresh accept, which
                 * lands asynchronously. Re-check on the next poll so a
                 * previous connection isn't re-accepted. */
                *revents = 0;
                return 0;
            }
            if (listener_status(s, stbuf, sizeof stbuf) == 0
                && strncmp(stbuf, "accepted", 8) == 0) {
                /* Stale-status guard: the status file keeps the last
                 * "accepted <N> ... <seq>" until netd replies again, so a
                 * bare prefix match re-reports readable for the same
                 * connection. Only a *new* seq is a fresh event. */
                int seq = status_accept_seq(stbuf);
                if (seq >= 0 && seq != s->last_accept_seq) {
                    rev |= POLLIN;
                } else {
                    /* We already consumed this accept. netd cleared its
                     * parked wait after the handoff, so it will not report
                     * the *next* connection until we ask again. Re-arm here
                     * or the server accepts exactly one connection then
                     * hangs on the listener forever. */
                    (void)listener_ctl(s, "accept");
                }
            }
        }
        *revents = rev;
        return rev ? 1 : 0;
    }

    /* Nonblocking connect: curl waits for POLLOUT (then SO_ERROR) until
     * netd advertises Established ("connected"). Do not report POLLOUT while
     * still SynSent / "connecting". */
    if (s->state == SOCK_CONNECTING) {
        int st = connect_status(s);
        if (st > 0) {
            finish_connect(s);
            if (want_out) {
                rev |= POLLOUT;
            }
            if (want_in && data_pending(s)) {
                rev |= POLLIN;
            }
            *revents = rev;
            return rev ? 1 : 0;
        }
        if (st < 0) {
            s->so_error = errno ? errno : ECONNREFUSED;
            s->state = SOCK_OPEN;
            if (s->ctl_fd >= 0) {
                close(s->ctl_fd);
                s->ctl_fd = -1;
            }
            rev |= POLLERR;
            if (want_out) {
                rev |= POLLOUT; /* wake curl to read SO_ERROR */
            }
            if (want_in) {
                rev |= POLLIN | POLLHUP;
            }
            *revents = rev;
            return 1;
        }
        *revents = 0;
        return 0;
    }

    /* Drain RX before surfacing hangup as the only POLLIN (half-close). */
    if (want_in && s->state == SOCK_CONNECTED && data_pending(s)) {
        rev |= POLLIN;
    }
    if (s->state == SOCK_CONNECTED && status_is_hangup(s)) {
        if (want_in) {
            rev |= POLLIN | POLLHUP;
        } else {
            rev |= POLLHUP;
        }
        if (want_out) {
            rev |= POLLOUT;
        }
        *revents = rev;
        return 1;
    }
    /* Connected TCP is writable unless we track a full TX buffer (we don't).
     * Never withhold POLLOUT when POLLIN is also requested: curl/mbedtls need
     * POLLOUT to send ClientHello while also watching for ServerHello. The old
     * withhold deadlocked HTTPS (curl:7 after ~15s in the connect/TLS phase).
     * Unconnected / connecting sockets must not report POLLOUT here. */
    if (want_out && s->state == SOCK_CONNECTED) {
        rev |= POLLOUT;
    }
    *revents = rev;
    return rev ? 1 : 0;
}

static int hangup_sock(struct myos_sock *s) {
    char path[64];
    int ctl;
    if (s->ctl_fd >= 0) {
        (void)write(s->ctl_fd, "hangup", 6);
        close(s->ctl_fd);
        s->ctl_fd = -1;
        return 0;
    }
    if (conv_path(path, sizeof path, s->proto_path, s->conv, "ctl") < 0) {
        return -1;
    }
    ctl = open(path, O_WRONLY);
    if (ctl >= 0) {
        (void)write(ctl, "hangup", 6);
        close(ctl);
    }
    return 0;
}

void myos_socket_on_close(int fd) {
    struct myos_sock *s = sock_by_fd(fd);
    if (s == NULL) {
        return;
    }
    /* No ctl hangup here: the kernel fires the netfs release (hangup) when
     * the LAST fd holder closes (fork-shared sockets: the parent's close
     * must not tear the connection down under the child). */
    (void)fd;
    s->data_fd = -1;
    sock_free(s);
}

int socket(int domain, int type, int protocol) {
    struct myos_sock *s;
    char clone_path[32];
    char path[64];
    char idbuf[16];
    ssize_t n;
    int clone_fd;
    int data_fd;
    int ctl_fd;
    unsigned short id;
    const char *proto;

    (void)protocol;

    if (domain != AF_INET) {
        errno = EAFNOSUPPORT;
        return -1;
    }
    if (type == SOCK_STREAM) {
        proto = "/net/tcp";
    } else if (type == SOCK_DGRAM) {
        proto = "/net/udp";
    } else {
        errno = EPROTONOSUPPORT;
        return -1;
    }

    s = sock_alloc();
    if (s == NULL) {
        errno = EMFILE;
        return -1;
    }

    {
        size_t pos = 0;
        int n = myos_cpy(clone_path, sizeof clone_path, proto);
        if (n < 0) { sock_free(s); errno = EIO; return -1; }
        pos = (size_t)n;
        if (pos + 6 >= sizeof clone_path) { sock_free(s); errno = EIO; return -1; }
        clone_path[pos++] = '/';
        myos_cpy(clone_path + pos, sizeof clone_path - pos, "clone");
    }
    clone_fd = open(clone_path, O_RDONLY);
    if (clone_fd < 0) {
        sock_free(s);
        errno = EIO;
        return -1;
    }
    n = read(clone_fd, idbuf, sizeof idbuf);
    close(clone_fd);
    if (n <= 0 || parse_clone_id(idbuf, (size_t)n, &id) < 0) {
        sock_free(s);
        errno = EIO;
        return -1;
    }

    if (conv_path(path, sizeof path, proto, id, "ctl") < 0) {
        sock_free(s);
        errno = EIO;
        return -1;
    }
    ctl_fd = open(path, O_RDWR);
    if (ctl_fd < 0) {
        /* try write-only */
        ctl_fd = open(path, O_WRONLY);
    }
    if (ctl_fd < 0) {
        sock_free(s);
        errno = EIO;
        return -1;
    }

    if (conv_path(path, sizeof path, proto, id, "data") < 0) {
        close(ctl_fd);
        sock_free(s);
        errno = EIO;
        return -1;
    }
    data_fd = open(path, O_RDWR);
    if (data_fd < 0) {
        close(ctl_fd);
        sock_free(s);
        errno = EIO;
        return -1;
    }

    strncpy(s->proto_path, proto, sizeof s->proto_path - 1);
    s->proto_path[sizeof s->proto_path - 1] = '\0';
    s->conv = id;
    s->ctl_fd = ctl_fd;
    s->data_fd = data_fd;
    s->type = type;
    s->state = SOCK_OPEN;
    return data_fd;
}

int bind(int sockfd, const struct sockaddr *addr, socklen_t addrlen) {
    struct myos_sock *s = sock_by_fd(sockfd);
    (void)addrlen;
    if (s == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    if (addr != NULL && addr->sa_family == AF_INET) {
        const struct sockaddr_in *in = (const struct sockaddr_in *)addr;
        /* INADDR_ANY / unspecified only (netd owns the single interface). */
        if (in->sin_addr.s_addr != INADDR_ANY && in->sin_addr.s_addr != 0) {
            errno = EOPNOTSUPP;
            return -1;
        }
        /* Remember the port; the listener is started by listen(). */
        s->bind_port = in->sin_port; /* already network order; netd wants text */
    }
    return 0;
}

/* Listener ctl helper: write a command to the conv's ctl file. */
static int listener_ctl(struct myos_sock *s, const char *cmd) {
    char path[64];
    int ctl;
    if (conv_path(path, sizeof path, s->proto_path, s->conv, "ctl") < 0) {
        return -1;
    }
    ctl = open(path, O_WRONLY);
    if (ctl < 0) {
        errno = EIO;
        return -1;
    }
    if (write(ctl, cmd, strlen(cmd)) < 0) {
        close(ctl);
        errno = EIO;
        return -1;
    }
    close(ctl);
    return 0;
}

/* Read the listener conv's status text ("listening" / "accepted <N> <ip>!<p>"). */
static int listener_status(struct myos_sock *s, char *out, size_t cap) {
    char path[64];
    char sbuf[80];
    int st;
    ssize_t nr;
    if (conv_path(path, sizeof path, s->proto_path, s->conv, "status") < 0) {
        return -1;
    }
    st = open(path, O_RDONLY);
    if (st < 0) {
        return -1;
    }
    nr = read(st, sbuf, sizeof sbuf - 1);
    close(st);
    if (nr <= 0) {
        return -1;
    }
    sbuf[nr] = '\0';
    size_t i = 0;
    while (i < (size_t)nr && (sbuf[i] == ' ' || sbuf[i] == '\n' || sbuf[i] == '\r')) {
        i++;
    }
    size_t o = 0;
    while (i < (size_t)nr && o + 1 < cap && sbuf[i] != '\n' && sbuf[i] != '\r') {
        out[o++] = sbuf[i++];
    }
    out[o] = '\0';
    return 0;
}

static int accept_from_status(struct myos_sock *ls, char *status,
    struct sockaddr *addr, socklen_t *addrlen);

int listen(int sockfd, int backlog) {
    struct myos_sock *s = sock_by_fd(sockfd);
    char cmd[32];
    (void)backlog; /* netd keeps a one-connection ready queue (smoltcp listener) */
    if (s == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    if (s->type != SOCK_STREAM) {
        errno = EOPNOTSUPP;
        return -1;
    }
    if (s->bind_port == 0) {
        errno = EINVAL;
        return -1;
    }
    unsigned short p = s->bind_port;
    unsigned short hp = (unsigned short)((p >> 8) | (p << 8)); /* net order -> host */
    /* Hand-format "announce <port>" via myos_u16_dec: pulling snprintf into
     * this TU drags newlib's float printf machinery into the riscv64
     * soft-float link (undefined __adddf3 et al.). */
    size_t cmd_pos = 0;
    {
        const char ann[] = "announce ";
        memcpy(cmd + cmd_pos, ann, sizeof ann - 1);
        cmd_pos += sizeof ann - 1;
    }
    {
        size_t used = myos_u16_dec(cmd + cmd_pos, sizeof cmd - cmd_pos - 1,
            (unsigned)hp);
        if (used == 0) {
            errno = EINVAL;
            return -1;
        }
        cmd_pos += used;
    }
    cmd[cmd_pos] = '\0';
    if (listener_ctl(s, cmd) < 0) {
        return -1;
    }
    s->state = SOCK_LISTENING;
    return 0;
}

int accept(int sockfd, struct sockaddr *addr, socklen_t *addrlen) {
    struct myos_sock *ls = sock_by_fd(sockfd);
    struct timeval start;
    if (ls == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    if (ls->state != SOCK_LISTENING) {
        errno = EINVAL;
        return -1;
    }
    if (ls->nonblock) {
        /* Nonblocking accept: one check, EAGAIN when nothing is pending. */
        char stbuf[80];
        if (!ls->accept_armed) {
            if (listener_ctl(ls, "accept") != 0) {
                return -1;
            }
            ls->accept_armed = 1;
        }
        if (listener_status(ls, stbuf, sizeof stbuf) < 0) {
            errno = EAGAIN;
            return -1;
        }
        if (strncmp(stbuf, "accepted", 8) != 0
            || status_accept_seq(stbuf) == ls->last_accept_seq) {
            errno = EAGAIN;
            return -1;
        }
        ls->accept_armed = 0;
        return accept_from_status(ls, stbuf, addr, addrlen);
    }
    if (gettimeofday(&start, NULL) != 0) {
        errno = EIO;
        return -1;
    }
    /* Blocking accept: arm netd once, then poll the listener status until it
     * reports "accepted <N>" (REP_STATUS lands asynchronously from netd). */
    if (!ls->accept_armed) {
        if (listener_ctl(ls, "accept") < 0) {
            return -1;
        }
        ls->accept_armed = 1;
    }
    for (;;) {
        char stbuf[80];
        if (listener_status(ls, stbuf, sizeof stbuf) == 0
            && strncmp(stbuf, "accepted", 8) == 0
            && status_accept_seq(stbuf) != ls->last_accept_seq) {
            ls->accept_armed = 0;
            return accept_from_status(ls, stbuf, addr, addrlen);
        }
        if (elapsed_ms(&start) >= 180000L) {
            errno = ETIMEDOUT;
            return -1;
        }
    }
}

/* Parse "accepted <N>[ <ip>!<port>][ <seq>]" and open the new conv as a socket fd. */
static int accept_from_status(struct myos_sock *ls, char *status,
    struct sockaddr *addr, socklen_t *addrlen) {
    char path[64];
    int data_fd;
    unsigned int n = 0;
    unsigned int seq = 0;
    const char *p = status + 8; /* skip "accepted" */
    while (*p == ' ') {
        p++;
    }
    if (*p < '0' || *p > '9') {
        errno = EIO;
        return -1;
    }
    while (*p >= '0' && *p <= '9') {
        n = n * 10 + (unsigned)(*p - '0');
        p++;
    }
    /* The trailing " <seq>" token is a per-listener monotonic counter; strip
     * it so the peer parser below sees the old "[ <ip>!<port>]" form. */
    {
        int L = (int)strlen(status);
        int j = L - 1;
        while (j >= 0 && status[j] != ' ') {
            j--;
        }
        if (j >= 0) {
            const char *t = status + j + 1;
            if (*t >= '0' && *t <= '9') {
                unsigned int v = 0;
                while (*t >= '0' && *t <= '9') {
                    v = v * 10 + (unsigned)(*t - '0');
                    t++;
                }
                if (*t == '\0') {
                    seq = v;
                    status[j] = '\0';
                }
            }
        }
    }
    ls->last_accept_seq = (int)seq;
    /* Take the parked handoff in netd now. Leaving `accepted` occupied holds
     * the next SYN on the listen handle (no Listen socket). While held,
     * older netd pumped that socket as the listener and could mark it
     * connected/hungup — wedging further accepts under concurrent dual-SYN.
     * Taking here frees the backlog slot so pump_accepts can move the
     * on-listen connection and re-arm Listen immediately. */
    (void)listener_ctl(ls, "accept");
    struct myos_sock *s = sock_alloc();
    if (s == NULL) {
        errno = EMFILE;
        return -1;
    }
    strncpy(s->proto_path, ls->proto_path, sizeof s->proto_path - 1);
    s->proto_path[sizeof s->proto_path - 1] = '\0';
    s->conv = (unsigned short)n;
    s->ctl_fd = -1;
    s->type = SOCK_STREAM;
    s->state = SOCK_CONNECTED;
    if (conv_path(path, sizeof path, s->proto_path, s->conv, "data") < 0) {
        sock_free(s);
        errno = EIO;
        return -1;
    }
    data_fd = open(path, O_RDWR);
    if (data_fd < 0) {
        sock_free(s);
        errno = EIO;
        return -1;
    }
    s->data_fd = data_fd;
    /* Optional peer from "accepted <N> <ip>!<port>" — best effort. */
    while (*p == ' ') {
        p++;
    }
    if (*p != '\0') {
        unsigned b[4] = {0, 0, 0, 0};
        int port = 0;
        int k = 0;
        int ok = 1;
        for (const char *q = p; *q; q++) {
            if (*q >= '0' && *q <= '9') {
                if (k == 4) {
                    port = port * 10 + (*q - '0');
                } else {
                    b[k] = b[k] * 10 + (*q - '0');
                }
            } else if (*q == '.') {
                k++;
                if (k > 3) {
                    ok = 0;
                    break;
                }
            } else if (*q == '!') {
                k = 4;
            } else {
                ok = 0;
                break;
            }
        }
        if (ok && k == 4) {
            s->peer_set = 1;
            memset(&s->peer, 0, sizeof s->peer);
            s->peer.sin_family = AF_INET;
            s->peer.sin_port = (unsigned short)((port >> 8) | ((port & 0xff) << 8));
            s->peer.sin_addr.s_addr = (b[0]) | ((unsigned)b[1] << 8)
                | ((unsigned)b[2] << 16) | ((unsigned)b[3] << 24);
            if (addr != NULL) {
                struct sockaddr_in sa;
                memset(&sa, 0, sizeof sa);
                sa.sin_family = AF_INET;
                sa.sin_port = (unsigned short)((port >> 8) | ((port & 0xff) << 8));
                sa.sin_addr.s_addr = (b[0]) | ((unsigned)b[1] << 8)
                    | ((unsigned)b[2] << 16) | ((unsigned)b[3] << 24);
                memcpy(addr, &sa, sizeof sa);
                if (addrlen != NULL) {
                    *addrlen = (socklen_t)sizeof sa;
                }
            }
        }
    }
    return data_fd;
}

int connect(int sockfd, const struct sockaddr *addr, socklen_t addrlen) {
    struct myos_sock *s = sock_by_fd(sockfd);
    const struct sockaddr_in *in;
    char cmd[48];
    char ip[INET_ADDRSTRLEN];
    int cm;

    (void)addrlen;
    if (s == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    if (s->state == SOCK_CONNECTED) {
        errno = EISCONN;
        return -1;
    }
    if (s->state == SOCK_CONNECTING) {
        errno = EALREADY;
        return -1;
    }
    if (addr == NULL || addr->sa_family != AF_INET) {
        errno = EAFNOSUPPORT;
        return -1;
    }
    in = (const struct sockaddr_in *)addr;
    if (inet_ntop(AF_INET, &in->sin_addr, ip, sizeof ip) == NULL) {
        errno = EINVAL;
        return -1;
    }
    {
        size_t pos = 0;
        int n;
        n = myos_cpy(cmd, sizeof cmd, "connect ");
        if (n < 0) { errno = EINVAL; return -1; }
        pos = (size_t)n;
        n = myos_cpy(cmd + pos, sizeof cmd - pos, ip);
        if (n < 0) { errno = EINVAL; return -1; }
        pos += (size_t)n;
        if (pos + 1 >= sizeof cmd) { errno = EINVAL; return -1; }
        cmd[pos++] = '!';
        n = myos_u16_dec(cmd + pos, sizeof cmd - pos, ntohs(in->sin_port));
        if (n < 0) { errno = EINVAL; return -1; }
        pos += (size_t)n;
        cm = (int)pos;
    }
    if (s->ctl_fd < 0) {
        errno = EBADF;
        return -1;
    }
    if (write(s->ctl_fd, cmd, (size_t)cm) < 0) {
        errno = EIO;
        return -1;
    }
    /* Stash peer early; peer_set stays 0 until Established (getpeername). */
    s->peer = *in;
    s->so_error = 0;

    if (s->nonblock) {
        int st = connect_status(s);
        if (st > 0) {
            /* UDP (and rare fast TCP): already Established after ctl write. */
            finish_connect(s);
            return 0;
        }
        if (st < 0) {
            return -1;
        }
        /* TCP handshake in progress — curl polls for POLLOUT / SO_ERROR. */
        s->state = SOCK_CONNECTING;
        errno = EINPROGRESS;
        return -1;
    }

    if (wait_connected(s) < 0) {
        return -1;
    }
    finish_connect(s);
    return 0;
}

int shutdown(int sockfd, int how) {
    struct myos_sock *s = sock_by_fd(sockfd);
    (void)how;
    if (s == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    hangup_sock(s);
    s->state = SOCK_OPEN;
    return 0;
}

int setsockopt(int sockfd, int level, int optname, const void *optval, socklen_t optlen) {
    (void)level;
    (void)optname;
    (void)optval;
    (void)optlen;
    if (sock_by_fd(sockfd) == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    /* Accept and ignore common options so curl/mbedtls keep going. */
    return 0;
}

int getsockopt(int sockfd, int level, int optname, void *optval, socklen_t *optlen) {
    struct myos_sock *s = sock_by_fd(sockfd);
    (void)level;
    if (s == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    if (optval == NULL || optlen == NULL) {
        errno = EINVAL;
        return -1;
    }
    if (optname == SO_ERROR) {
        if (*optlen < sizeof(int)) {
            errno = EINVAL;
            return -1;
        }
        /* If still connecting, sample status once so a racing Established is
         * visible before curl reads SO_ERROR after POLLOUT. */
        if (s->state == SOCK_CONNECTING) {
            int st = connect_status(s);
            if (st > 0) {
                finish_connect(s);
            } else if (st < 0) {
                s->so_error = errno ? errno : ECONNREFUSED;
                s->state = SOCK_OPEN;
                if (s->ctl_fd >= 0) {
                    close(s->ctl_fd);
                    s->ctl_fd = -1;
                }
            }
        }
        *(int *)optval = s->so_error;
        *optlen = sizeof(int);
        s->so_error = 0;
        return 0;
    }
    if (optname == SO_TYPE) {
        if (*optlen < sizeof(int)) {
            errno = EINVAL;
            return -1;
        }
        *(int *)optval = s->type;
        *optlen = sizeof(int);
        return 0;
    }
    errno = ENOPROTOOPT;
    return -1;
}

int getsockname(int sockfd, struct sockaddr *addr, socklen_t *addrlen) {
    struct sockaddr_in local;
    if (sock_by_fd(sockfd) == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    if (addr == NULL || addrlen == NULL) {
        errno = EINVAL;
        return -1;
    }
    memset(&local, 0, sizeof local);
    local.sin_family = AF_INET;
    local.sin_addr.s_addr = INADDR_ANY;
    local.sin_port = 0;
    if (*addrlen > sizeof local) {
        *addrlen = sizeof local;
    }
    memcpy(addr, &local, *addrlen);
    return 0;
}

int getpeername(int sockfd, struct sockaddr *addr, socklen_t *addrlen) {
    struct myos_sock *s = sock_by_fd(sockfd);
    if (s == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    if (!s->peer_set) {
        errno = ENOTCONN;
        return -1;
    }
    if (addr == NULL || addrlen == NULL) {
        errno = EINVAL;
        return -1;
    }
    if (*addrlen > sizeof s->peer) {
        *addrlen = sizeof s->peer;
    }
    memcpy(addr, &s->peer, *addrlen);
    return 0;
}

ssize_t send(int sockfd, const void *buf, size_t len, int flags) {
    (void)flags;
    if (sock_by_fd(sockfd) == NULL) {
        /* Allow plain write path if somehow untracked; still try write. */
    }
    return write(sockfd, buf, len);
}

ssize_t recv(int sockfd, void *buf, size_t len, int flags) {
    ssize_t n;
    struct myos_sock *s = sock_by_fd(sockfd);
    int restore = 0;
    /* MSG_DONTWAIT: force nonblock for this call only (read path checks s->nonblock). */
    if (s != NULL && (flags & MSG_DONTWAIT) && !s->nonblock) {
        s->nonblock = 1;
        restore = 1;
    }
    n = read(sockfd, buf, len);
    if (restore) {
        s->nonblock = 0;
    }
    /* _read already maps empty connected reads (EAGAIN / block / hangup EOF). */
    (void)flags;
    return n;
}

ssize_t sendto(int sockfd, const void *buf, size_t len, int flags,
    const struct sockaddr *dest_addr, socklen_t addrlen) {
    struct myos_sock *s = sock_by_fd(sockfd);
    (void)flags;
    if (s != NULL && s->state != SOCK_CONNECTED && dest_addr != NULL) {
        if (connect(sockfd, dest_addr, addrlen) < 0) {
            return -1;
        }
    }
    return write(sockfd, buf, len);
}

ssize_t recvfrom(int sockfd, void *buf, size_t len, int flags,
    struct sockaddr *src_addr, socklen_t *addrlen) {
    ssize_t n;
    struct myos_sock *s;
    int restore = 0;
    s = sock_by_fd(sockfd);
    if (s != NULL && (flags & MSG_DONTWAIT) && !s->nonblock) {
        s->nonblock = 1;
        restore = 1;
    }
    n = read(sockfd, buf, len);
    if (restore) {
        s->nonblock = 0;
    }
    if (n >= 0 && src_addr != NULL && addrlen != NULL) {
        s = sock_by_fd(sockfd);
        if (s != NULL && s->peer_set) {
            if (*addrlen > sizeof s->peer) {
                *addrlen = sizeof s->peer;
            }
            memcpy(src_addr, &s->peer, *addrlen);
        }
    }
    (void)flags;
    return n;
}
