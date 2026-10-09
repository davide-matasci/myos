/*
 * Userspace BSD sockets over Plan 9 /net + netd (smoltcp).
 * No socket() syscall — TCP and UDP via clone/ctl/data, and AF_UNIX
 * stream sockets over /net/unix, which the kernel serves itself
 * (docs/sockets-unix.md).
 *
 * A UDP socket keeps its ctl open. Unconnected, its conversation is in
 * netfs's "headers" mode: each datagram read or written carries netd's
 * 12-byte header (remote address, local address, remote port, local port:
 * Plan 9's udp headers), which recvfrom and sendto turn into addresses.
 * Connected, it reads and writes bare datagrams, so a program it is passed
 * to by exec (which knows nothing of the socket) still can. Binding,
 * connecting and their undoing are netd's (udp_ctl); an error netd reports
 * (a refused datagram) fails the next read or write and is SO_ERROR.
 */
#include <errno.h>
#include <fcntl.h>
#include "myos_fmt.h"
#include <poll.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <sys/un.h>

#include "myos_syscalls.h"

#include <signal.h>

/* As many sockets as fds: the fd limit is the only one (an X server takes a
 * socket per client). */
#define MYOS_MAX_SOCKS MYOS_OPEN_MAX
#define UN_NAME_CAP ((int)sizeof(((struct sockaddr_un *)0)->sun_path))
#define CONNECT_TIMEOUT_MS 30000
/* A UDP datagram's header to and from netd, and the most one netfs
 * message carries behind it. */
#define UDP_HDR 12
#define UDP_MAX 2030
/* How long netd may take to answer a UDP ctl command. */
#define UDP_CTL_TIMEOUT_MS 5000

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
    int taken_seq;     /* listener: seq named by the last "taken <seq>" sent */
    struct timeval taken_tv; /* listener: when that "taken" was sent */
    int family;     /* AF_INET or AF_UNIX */
    char un_name[UN_NAME_CAP + 1]; /* AF_UNIX: bound (listener's) name */
    char un_peer[UN_NAME_CAP + 1]; /* AF_UNIX: the name connected to */
    /* UDP: */
    int headers;    /* netfs shows datagram headers (unconnected) */
    struct sockaddr_in local; /* local address and port, netd's answers */
    int shut;       /* shutdown(): SHUT_RD_BIT, SHUT_WR_BIT */
    int reuse;      /* SO_REUSEADDR, for bind */
    int broadcast;  /* SO_BROADCAST (kept for getsockopt) */
    unsigned ctl_tag; /* the last udp_ctl command's tag */
};

#define SHUT_RD_BIT 1
#define SHUT_WR_BIT 2

static struct myos_sock socks[MYOS_MAX_SOCKS];
/* The fds of UDP sockets: read and write are theirs (dgram_read/write),
 * a look up that every read and write of a program with sockets makes. */
static unsigned char dgram_fds[MYOS_MAX_SOCKS];

static int is_dgram(int fd) {
    return fd >= 0 && fd < MYOS_MAX_SOCKS && dgram_fds[fd];
}

static int udp_bind(struct myos_sock *s, const struct sockaddr_in *in);
static int udp_connect(struct myos_sock *s, const struct sockaddr *addr, socklen_t addrlen);
static int udp_take_error(struct myos_sock *s);
static int udp_set_headers(struct myos_sock *s, int on);
static ssize_t udp_send(struct myos_sock *s, const void *buf, size_t len, int flags,
    const struct sockaddr *dest, socklen_t destlen);
static ssize_t udp_recv(struct myos_sock *s, void *buf, size_t len, int flags,
    struct sockaddr *from, socklen_t *fromlen);

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
            socks[i].taken_seq = -1;
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

/* Block until `fd` has `events` (or a hangup / error) for at most
 * `timeout_ms` (-1: no limit): the kernel's poll, which sleeps until netfs
 * reports a change. 1 = ready, 0 = timed out, -1 = interrupted (EINTR). */
static int sock_wait(int fd, short events, int timeout_ms) {
    struct pollfd p = {fd, events, 0};
    return __myos_kpoll(&p, 1, timeout_ms);
}

/* What is left of `total_ms` since `start` (0 once past). */
static int ms_left(const struct timeval *start, long total_ms) {
    long left = total_ms - elapsed_ms(start);
    return left < 0 ? 0 : (int)left;
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
        /* netfs reports POLLOUT once connected, POLLHUP on failure. */
        if (sock_wait(s->data_fd, POLLOUT, ms_left(&start, CONNECT_TIMEOUT_MS)) < 0) {
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

/* What a read would find: 0 = data, 1 = end of file (hangup, nothing left),
 * 2 = nothing yet. The hangup is looked at first: netd sends it after the
 * last data, and both can land between two looks, so "no data" followed by
 * "hangup" would be an end of file with that data still unread (curl: "end
 * of response with N bytes missing", once the kernel's poll woke readers
 * on the very write of the last data). */
static int rx_state(struct myos_sock *s) {
    int hangup = status_is_hangup(s);
    if (data_pending(s)) {
        return 0;
    }
    return hangup ? 1 : 2;
}

/* Block until RX data or hangup. Returns 0 = data ready, 1 = hangup,
 * -1 = interrupted (errno EINTR). */
static int wait_readable(struct myos_sock *s) {
    for (;;) {
        /* Drain remaining RX before treating hangup as EOF (BSD half-close). */
        int st = rx_state(s);
        if (st != 2) {
            return st;
        }
        if (sock_wait(s->data_fd, POLLIN, -1) < 0) {
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
 *   4 = the wait was interrupted -> EINTR
 */
int myos_socket_empty_read(int fd) {
    struct myos_sock *s = sock_by_fd(fd);
    int wr;
    if (s == NULL || s->state != SOCK_CONNECTED) {
        return 0;
    }
    /* Prefer pending RX over hangup (BSD half-close). _read returned 0, but
     * REP_DATA may have landed after the syscall; never EOF while st_size > 0. */
    switch (rx_state(s)) {
    case 0:
        return 3;
    case 1:
        return 2;
    }
    if (s->nonblock) {
        return 1;
    }
    /* BSD: empty read on a blocking TCP socket waits for data or hangup. */
    wr = wait_readable(s);
    if (wr < 0) {
        return 4;
    }
    return wr == 1 ? 2 : 3;
}

/*
 * Called from _write when the kernel refused a write on a tracked socket fd.
 * A stream refuses a write when it has no room: a unix peer's buffer is
 * full, or netd has a TCP conversation's 8 KiB of sends still queued.
 *   0 = not handled (keep the generic error)
 *   1 = no room + nonblocking -> EAGAIN
 *   2 = peer gone -> EPIPE
 *   3 = waited for room: retry the write
 *   4 = not connected -> ENOTCONN
 *   5 = the wait was interrupted -> EINTR
 */
int myos_socket_write_failed(int fd) {
    struct myos_sock *s = sock_by_fd(fd);
    struct pollfd p = {fd, POLLOUT, 0};
    if (s == NULL || s->type != SOCK_STREAM) {
        return 0;
    }
    if (s->state != SOCK_CONNECTED) {
        return 4;
    }
    if (status_is_hangup(s)) {
        return 2;
    }
    if (s->nonblock) {
        return 1;
    }
    /* netfs reports POLLOUT once there is room again; a hangup (or a
     * conversation gone) ends the wait too. */
    if (__myos_kpoll(&p, 1, -1) < 0) {
        return 5;
    }
    return p.revents & (POLLHUP | POLLERR | POLLNVAL) ? 2 : 3;
}

/* Whether a short write on `fd` should go on with the rest: a blocking
 * connected stream (POSIX: it returns once everything is written). */
int myos_socket_write_all(int fd) {
    struct myos_sock *s = sock_by_fd(fd);
    return s != NULL && s->type == SOCK_STREAM && s->state == SOCK_CONNECTED && !s->nonblock;
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
static void listener_retry_taken(struct myos_sock *ls, int seq);
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

/*
 * poll() of a tracked socket, around the kernel call (pollselect.c). The
 * kernel knows what netfs knows: bytes to read, a hangup, a connect done
 * (POLLOUT once "connected"), an accept not taken yet, room in a unix
 * peer's buffer. This adds the socket's own state.
 *
 * prepare: returns -1 for a non-socket. Sets *now to what is ready without
 * asking (a connected UDP socket is always writable) and *kevents to what
 * the kernel should wait for (a stream is writable while it has room). A listener
 * arms netd's accept first: select-driven servers like dropbear select()
 * before accept(), and netd only announces a connection once armed.
 */
int myos_socket_poll_prepare(int fd, short events, short *now, short *kevents) {
    struct myos_sock *s = sock_by_fd(fd);
    short want_in;
    short want_out;
    if (s == NULL) {
        return -1;
    }
    want_in = events & (POLLIN | POLLPRI | POLLRDNORM);
    want_out = events & (POLLOUT | POLLWRNORM);
    *now = 0;
    *kevents = 0;
    if (is_dgram(fd)) {
        /* A datagram can be sent at any time; after SHUT_RD a read ends at
         * once. netfs adds POLLERR for an error to take. */
        *now = (want_out ? POLLOUT : 0) | ((s->shut & SHUT_RD_BIT) ? want_in : 0);
        *kevents = want_in ? POLLIN : 0;
        return 0;
    }
    switch (s->state) {
    case SOCK_LISTENING:
        if (want_in && s->family != AF_UNIX && !s->accept_armed
            && listener_ctl(s, "accept") == 0) {
            s->accept_armed = 1;
        }
        *kevents = want_in ? POLLIN : 0;
        break;
    case SOCK_CONNECTING:
        /* POLLOUT (or a hangup) tells the connect is over. */
        *kevents = POLLOUT | (want_in ? POLLIN : 0);
        break;
    case SOCK_CONNECTED:
        if (s->type == SOCK_STREAM) {
            *kevents = (want_in ? POLLIN : 0) | (want_out ? POLLOUT : 0);
        } else {
            *now = want_out ? POLLOUT : 0;
            *kevents = want_in ? POLLIN : 0;
        }
        break;
    default:
        /* Not connected: nothing to wait for but a hangup. */
        break;
    }
    return 0;
}

/* done: the kernel's revents for a tracked socket become the socket's. */
void myos_socket_poll_done(int fd, short events, short *revents) {
    struct myos_sock *s = sock_by_fd(fd);
    short want_in;
    short want_out;
    short rev;
    if (s == NULL || is_dgram(fd)) {
        return;
    }
    want_in = events & (POLLIN | POLLPRI | POLLRDNORM);
    want_out = events & (POLLOUT | POLLWRNORM);
    rev = *revents;
    if (s->state == SOCK_LISTENING && s->family != AF_UNIX && (rev & POLLIN)) {
        /* Stale-status guard: the status file keeps the last "accepted <N>
         * ... <seq>" until netd replies again; only a seq we have not
         * accepted yet is a new connection. Otherwise netd has not processed
         * our "taken <seq>" (or it was lost when the netfs ring was full):
         * re-send it, rate-limited, never "accept" (that flooded the ring
         * and failed dropbear's session writes with EIO). */
        char stbuf[80];
        int seq = -1;
        if (listener_status(s, stbuf, sizeof stbuf) == 0
            && strncmp(stbuf, "accepted", 8) == 0) {
            seq = status_accept_seq(stbuf);
        }
        if (seq < 0 || seq == s->last_accept_seq) {
            listener_retry_taken(s, seq);
            rev &= ~POLLIN;
        }
    } else if (s->state == SOCK_CONNECTING) {
        if (rev & (POLLHUP | POLLERR)) {
            /* Refused or failed: curl reads SO_ERROR after POLLOUT. */
            s->so_error = ECONNREFUSED;
            s->state = SOCK_OPEN;
            if (s->ctl_fd >= 0) {
                close(s->ctl_fd);
                s->ctl_fd = -1;
            }
            rev = POLLERR | (want_out ? POLLOUT : 0) | (want_in ? POLLIN | POLLHUP : 0);
        } else if (rev & POLLOUT) {
            finish_connect(s);
            rev &= (want_out ? POLLOUT : 0) | (want_in ? POLLIN : 0);
        }
    } else if (s->state == SOCK_CONNECTED && (rev & POLLHUP) && want_out) {
        /* A hung-up socket is "writable": the write reports the error. */
        rev |= POLLOUT;
    }
    *revents = rev;
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
     * must not tear the connection down under the child). A listener
     * still holds its ctl fd: closing that tears nothing down (netfs acts
     * on the last data close only), it just stops leaking the fd. */
    if (is_dgram(fd)) {
        dgram_fds[fd] = 0;
    }
    if (s->ctl_fd >= 0) {
        close(s->ctl_fd);
    }
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

    if (domain == AF_UNIX) {
        if (type != SOCK_STREAM) {
            errno = EPROTONOSUPPORT;
            return -1;
        }
        proto = "/net/unix";
    } else if (domain != AF_INET) {
        errno = EAFNOSUPPORT;
        return -1;
    } else if (type == SOCK_STREAM) {
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
    s->family = domain;
    s->state = SOCK_OPEN;
    if (type == SOCK_DGRAM) {
        s->local.sin_family = AF_INET;
        if (data_fd >= MYOS_MAX_SOCKS || udp_set_headers(s, 1) < 0) {
            close(data_fd);
            errno = EMFILE;
            return -1;
        }
        dgram_fds[data_fd] = 1;
    }
    return data_fd;
}

/* AF_UNIX: the name in `addr` (sun_path up to its NUL, within addrlen) as
 * /net/unix text. An abstract name (leading NUL) becomes "@name". */
static int un_name_of(const struct sockaddr *addr, socklen_t addrlen, char *out) {
    const char *path = ((const struct sockaddr_un *)addr)->sun_path;
    size_t len;
    size_t i = 0;
    size_t j = 0;
    if (addrlen <= offsetof(struct sockaddr_un, sun_path)) {
        return -1;
    }
    len = addrlen - offsetof(struct sockaddr_un, sun_path);
    if (len > (size_t)UN_NAME_CAP) {
        len = UN_NAME_CAP;
    }
    if (path[0] == '\0') {
        out[i++] = '@';
        j = 1;
    }
    for (; j < len && path[j] != '\0'; j++) {
        out[i++] = path[j];
    }
    out[i] = '\0';
    return (i == 0 || (i == 1 && out[0] == '@')) ? -1 : 0;
}

/* AF_UNIX: fill `addr` with `name` (empty: an unnamed socket). */
static void un_put_name(const char *name, struct sockaddr *addr, socklen_t *addrlen) {
    struct sockaddr_un un;
    size_t n = strlen(name);
    socklen_t len = (socklen_t)(offsetof(struct sockaddr_un, sun_path) + (n ? n + 1 : 0));
    if (addr == NULL || addrlen == NULL) {
        return;
    }
    memset(&un, 0, sizeof un);
    un.sun_family = AF_UNIX;
    memcpy(un.sun_path, name, n);
    if (name[0] == '@') {
        un.sun_path[0] = '\0'; /* abstract */
        len--;
    }
    memcpy(addr, &un, *addrlen < len ? *addrlen : len);
    *addrlen = len;
}

int bind(int sockfd, const struct sockaddr *addr, socklen_t addrlen) {
    struct myos_sock *s = sock_by_fd(sockfd);
    if (s == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    if (addr != NULL && s->family == AF_UNIX) {
        /* The name is announced by listen(). */
        if (addr->sa_family != AF_UNIX) {
            errno = EAFNOSUPPORT;
            return -1;
        }
        if (un_name_of(addr, addrlen, s->un_name) < 0) {
            errno = EINVAL;
            return -1;
        }
        return 0;
    }
    if (s->type == SOCK_DGRAM) {
        if (addr == NULL || addrlen < sizeof(struct sockaddr_in)) {
            errno = EINVAL;
            return -1;
        }
        if (addr->sa_family != AF_INET) {
            errno = EAFNOSUPPORT;
            return -1;
        }
        return udp_bind(s, (const struct sockaddr_in *)addr);
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

/* Unsigned decimal (seq values outgrow myos_u16_dec). */
static int myos_u32_dec(char *dst, size_t cap, unsigned v) {
    char tmp[10];
    int i = 10;
    int n;
    do {
        tmp[--i] = (char)('0' + (v % 10u));
        v /= 10u;
    } while (v != 0 && i > 0);
    n = 10 - i;
    if ((size_t)n >= cap) {
        return -1;
    }
    memcpy(dst, tmp + i, (size_t)n);
    dst[n] = '\0';
    return n;
}

/* Tell netd the handoff <seq> was consumed (see listener_retry_taken). */
static void listener_send_taken(struct myos_sock *ls, int seq) {
    char cmd[24];
    const char pfx[] = "taken ";
    memcpy(cmd, pfx, sizeof pfx - 1);
    if (myos_u32_dec(cmd + sizeof pfx - 1, sizeof cmd - (sizeof pfx - 1),
            (unsigned)seq) < 0) {
        return;
    }
    ls->taken_seq = seq;
    (void)gettimeofday(&ls->taken_tv, NULL);
    (void)listener_ctl(ls, cmd);
}

/* The listener status still advertises handoff <seq>, which we consumed:
 * netd has not processed our "taken <seq>" yet, or the write failed (netfs
 * request ring full) and it never will. netd ignores a "taken" whose seq is
 * no longer the head, so re-sending is safe; limit it to every 100ms so a
 * select() spin cannot flood the ring. Without the retry a lost "taken" left
 * the consumed head parked and every later connection unannounced. */
static void listener_retry_taken(struct myos_sock *ls, int seq) {
    if (seq < 0 || seq != ls->last_accept_seq) {
        return;
    }
    if (ls->taken_seq == seq && elapsed_ms(&ls->taken_tv) < 100) {
        return;
    }
    listener_send_taken(ls, seq);
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

/* AF_UNIX: take the next connection queued on `s` (its listen file).
 * 1 = *id is it, 0 = none waiting, -1 = error. */
static int un_take(struct myos_sock *s, unsigned short *id) {
    char path[64];
    char buf[16];
    ssize_t n;
    int fd;
    if (conv_path(path, sizeof path, s->proto_path, s->conv, "listen") < 0) {
        errno = EIO;
        return -1;
    }
    fd = open(path, O_RDONLY);
    if (fd < 0) {
        errno = EIO;
        return -1;
    }
    n = read(fd, buf, sizeof buf);
    close(fd);
    if (n <= 0) {
        return 0;
    }
    return parse_clone_id(buf, (size_t)n, id) == 0 ? 1 : (errno = EIO, -1);
}

/* AF_UNIX: open conversation <id> (connected) as a socket fd. */
static int un_open(unsigned short id, const char *name) {
    char path[64];
    struct myos_sock *s = sock_alloc();
    if (s == NULL) {
        errno = EMFILE;
        return -1;
    }
    strcpy(s->proto_path, "/net/unix");
    s->conv = id;
    s->type = SOCK_STREAM;
    s->family = AF_UNIX;
    s->state = SOCK_CONNECTED;
    s->peer_set = 1;
    strcpy(s->un_name, name);
    if (conv_path(path, sizeof path, s->proto_path, id, "data") < 0
        || (s->data_fd = open(path, O_RDWR)) < 0) {
        sock_free(s);
        errno = EIO;
        return -1;
    }
    return s->data_fd;
}

/* AF_UNIX accept: connections wait on the listener's queue in the kernel;
 * a blocking accept sleeps between looks. The peer is unnamed. */
static int un_accept(struct myos_sock *ls, struct sockaddr *addr, socklen_t *addrlen) {
    unsigned short id;
    int fd;
    for (;;) {
        int r = un_take(ls, &id);
        if (r < 0) {
            return -1;
        }
        if (r > 0) {
            break;
        }
        if (ls->nonblock) {
            errno = EAGAIN;
            return -1;
        }
        /* netfs reports POLLIN once a connection is queued. */
        if (sock_wait(ls->data_fd, POLLIN, -1) < 0) {
            return -1;
        }
    }
    fd = un_open(id, ls->un_name);
    if (fd >= 0) {
        un_put_name("", addr, addrlen);
    }
    return fd;
}

int socketpair(int domain, int type, int protocol, int sv[2]) {
    struct myos_sock *s;
    unsigned short id;
    int a;
    int b;
    if (domain != AF_UNIX) {
        errno = EAFNOSUPPORT;
        return -1;
    }
    if (type != SOCK_STREAM) {
        errno = EPROTONOSUPPORT;
        return -1;
    }
    a = socket(AF_UNIX, SOCK_STREAM, protocol);
    if (a < 0) {
        return -1;
    }
    s = sock_by_fd(a);
    /* "pair" connects a new conversation to this one and queues it on our
     * listen file, where un_take picks it up like an accepted connection. */
    if (write(s->ctl_fd, "pair", 4) < 0 || un_take(s, &id) <= 0) {
        close(a);
        errno = EMFILE;
        return -1;
    }
    finish_connect(s);
    b = un_open(id, "");
    if (b < 0) {
        close(a);
        return -1;
    }
    sv[0] = a;
    sv[1] = b;
    return 0;
}

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
    if (s->family == AF_UNIX) {
        char ucmd[sizeof "announce " + UN_NAME_CAP];
        if (s->un_name[0] == '\0') {
            errno = EINVAL;
            return -1;
        }
        memcpy(ucmd, "announce ", 9);
        strcpy(ucmd + 9, s->un_name);
        if (listener_ctl(s, ucmd) < 0) {
            errno = EADDRINUSE; /* the name has a listener already */
            return -1;
        }
        s->state = SOCK_LISTENING;
        return 0;
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

static int accept_body(int sockfd, struct sockaddr *addr, socklen_t *addrlen) {
    struct myos_sock *ls = sock_by_fd(sockfd);
    struct timeval start;
    if (ls == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    if (ls->type == SOCK_DGRAM) {
        errno = EOPNOTSUPP;
        return -1;
    }
    if (ls->state != SOCK_LISTENING) {
        errno = EINVAL;
        return -1;
    }
    if (ls->family == AF_UNIX) {
        return un_accept(ls, addr, addrlen);
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
        if (strncmp(stbuf, "accepted", 8) != 0) {
            errno = EAGAIN;
            return -1;
        }
        if (status_accept_seq(stbuf) == ls->last_accept_seq) {
            listener_retry_taken(ls, ls->last_accept_seq);
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
    /* Blocking accept: arm netd once, then wait for the listener status to
     * report "accepted <N>" (REP_STATUS lands asynchronously from netd;
     * netfs reports POLLIN for an accept not yet taken). */
    if (!ls->accept_armed) {
        if (listener_ctl(ls, "accept") < 0) {
            return -1;
        }
        ls->accept_armed = 1;
    }
    for (;;) {
        char stbuf[80];
        if (listener_status(ls, stbuf, sizeof stbuf) == 0
            && strncmp(stbuf, "accepted", 8) == 0) {
            if (status_accept_seq(stbuf) != ls->last_accept_seq) {
                ls->accept_armed = 0;
                return accept_from_status(ls, stbuf, addr, addrlen);
            }
            listener_retry_taken(ls, ls->last_accept_seq);
        }
        if (elapsed_ms(&start) >= 180000L) {
            errno = ETIMEDOUT;
            return -1;
        }
        if (sock_wait(ls->data_fd, POLLIN, ms_left(&start, 180000L)) < 0) {
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
    s->family = AF_INET;
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
    /* Consume the parked head in netd without re-advertising it. A plain
     * ctl "accept" would take+reply the same <N> (and with accepted_pending
     * bump seq), so the next accept() re-opened the live Child — dropbear
     * Integrity error bad packet size 0x53534831 on sequential SSH. */
    listener_send_taken(ls, (int)seq);
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

static int connect_body(int sockfd, const struct sockaddr *addr, socklen_t addrlen) {
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
    if (s->type == SOCK_DGRAM) {
        return udp_connect(s, addr, addrlen);
    }
    if (s->state == SOCK_CONNECTED) {
        errno = EISCONN;
        return -1;
    }
    if (s->state == SOCK_CONNECTING) {
        errno = EALREADY;
        return -1;
    }
    if (addr == NULL || addr->sa_family != s->family) {
        errno = EAFNOSUPPORT;
        return -1;
    }
    if (s->family == AF_UNIX) {
        /* Local: connected (queued on the listener) or refused at once. */
        char ucmd[sizeof "connect " + UN_NAME_CAP];
        if (un_name_of(addr, addrlen, s->un_peer) < 0) {
            errno = EINVAL;
            return -1;
        }
        memcpy(ucmd, "connect ", 8);
        strcpy(ucmd + 8, s->un_peer);
        if (s->ctl_fd < 0 || write(s->ctl_fd, ucmd, strlen(ucmd)) < 0) {
            errno = ECONNREFUSED; /* no listener of that name, or its queue is full */
            return -1;
        }
        finish_connect(s);
        return 0;
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
    if (s->type == SOCK_DGRAM) {
        /* As on Linux: reads end, writes fail (EPIPE), and an unconnected
         * socket says ENOTCONN but takes it all the same. */
        if (how != SHUT_RD && how != SHUT_WR && how != SHUT_RDWR) {
            errno = EINVAL;
            return -1;
        }
        s->shut |= how == SHUT_RD ? SHUT_RD_BIT
            : how == SHUT_WR ? SHUT_WR_BIT : SHUT_RD_BIT | SHUT_WR_BIT;
        if (s->state != SOCK_CONNECTED) {
            errno = ENOTCONN;
            return -1;
        }
        return 0;
    }
    hangup_sock(s);
    s->state = SOCK_OPEN;
    return 0;
}

int setsockopt(int sockfd, int level, int optname, const void *optval, socklen_t optlen) {
    struct myos_sock *s = sock_by_fd(sockfd);
    if (s == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    /* Kept for a UDP bind (SO_REUSEADDR) and getsockopt; the others are
     * accepted and ignored so curl/mbedtls keep going. */
    if (level == SOL_SOCKET && (optname == SO_REUSEADDR || optname == SO_BROADCAST)) {
        if (optval == NULL || optlen < sizeof(int)) {
            errno = EINVAL;
            return -1;
        }
        *(optname == SO_REUSEADDR ? &s->reuse : &s->broadcast) = *(const int *)optval != 0;
    }
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
        if (s->so_error == 0 && s->type == SOCK_DGRAM) {
            s->so_error = udp_take_error(s);
        }
        *(int *)optval = s->so_error;
        *optlen = sizeof(int);
        s->so_error = 0;
        return 0;
    }
    if (optname == SO_REUSEADDR || optname == SO_BROADCAST) {
        if (*optlen < sizeof(int)) {
            errno = EINVAL;
            return -1;
        }
        *(int *)optval = optname == SO_REUSEADDR ? s->reuse : s->broadcast;
        *optlen = sizeof(int);
        return 0;
    }
    if (optname == SO_BINDTODEVICE) {
        /* Bound to no device: the empty name. */
        *optlen = 0;
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
    struct myos_sock *s = sock_by_fd(sockfd);
    if (s == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    if (addr == NULL || addrlen == NULL) {
        errno = EINVAL;
        return -1;
    }
    if (s->family == AF_UNIX) {
        un_put_name(s->un_name, addr, addrlen);
        return 0;
    }
    memset(&local, 0, sizeof local);
    local.sin_family = AF_INET;
    local.sin_addr.s_addr = INADDR_ANY;
    local.sin_port = 0;
    if (s->type == SOCK_DGRAM) {
        local = s->local;
    }
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
    /* A UDP socket connected to port 0 has no peer, as on Linux. */
    if (!s->peer_set || (s->type == SOCK_DGRAM && s->peer.sin_port == 0)) {
        errno = ENOTCONN;
        return -1;
    }
    if (addr == NULL || addrlen == NULL) {
        errno = EINVAL;
        return -1;
    }
    if (s->family == AF_UNIX) {
        un_put_name(s->un_peer, addr, addrlen);
        return 0;
    }
    if (*addrlen > sizeof s->peer) {
        *addrlen = sizeof s->peer;
    }
    memcpy(addr, &s->peer, *addrlen);
    return 0;
}

ssize_t send(int sockfd, const void *buf, size_t len, int flags) {
    if (is_dgram(sockfd)) {
        return udp_send(sock_by_fd(sockfd), buf, len, flags, NULL, 0);
    }
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
    if (is_dgram(sockfd)) {
        return udp_recv(s, buf, len, flags, NULL, NULL);
    }
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
    if (is_dgram(sockfd)) {
        return udp_send(s, buf, len, flags, dest_addr, addrlen);
    }
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
    if (is_dgram(sockfd)) {
        return udp_recv(s, buf, len, flags, src_addr, addrlen);
    }
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

/* Cancellation points (pthread.c): accept and connect wait for netd. */
int accept(int sockfd, struct sockaddr *addr, socklen_t *addrlen) {
    __myos_cancel_enter();
    int r = accept_body(sockfd, addr, addrlen);
    __myos_cancel_leave();
    return r;
}

int connect(int sockfd, const struct sockaddr *addr, socklen_t addrlen) {
    __myos_cancel_enter();
    int r = connect_body(sockfd, addr, addrlen);
    __myos_cancel_leave();
    return r;
}

/* ---- UDP (SOCK_DGRAM over /net/udp; see the notes at the top) ---- */

/* errno for netd's word for what went wrong. */
static int udp_errno(const char *why) {
    static const struct {
        const char *why;
        int err;
    } words[] = {
        {"addrinuse", EADDRINUSE},
        {"addrnotavail", EADDRNOTAVAIL},
        {"netunreach", ENETUNREACH},
        {"refused", ECONNREFUSED},
    };
    size_t n = strcspn(why, " \n");
    size_t i;
    for (i = 0; i < sizeof words / sizeof words[0]; i++) {
        if (strlen(words[i].why) == n && memcmp(why, words[i].why, n) == 0) {
            return words[i].err;
        }
    }
    return EINVAL;
}

/* netfs shows (on: an unconnected socket) or hides datagram headers. */
static int udp_set_headers(struct myos_sock *s, int on) {
    const char *cmd = on ? "headers" : "noheaders";
    if (write(s->ctl_fd, cmd, strlen(cmd)) < 0) {
        errno = EIO;
        return -1;
    }
    s->headers = on;
    return 0;
}

/* The error netd reported for `s`, taken (reading ctl clears it); 0 when
 * there is none. */
static int udp_take_error(struct myos_sock *s) {
    char b[24];
    ssize_t n = pread(s->ctl_fd, b, sizeof b - 1, 0);
    if (n <= 0) {
        return 0;
    }
    b[n] = '\0';
    return udp_errno(b);
}

/* netd's answer to a udp_ctl command: "ok <addr>!<port>", the local
 * address after it, or "fail <why>". */
static int udp_answer(struct myos_sock *s, const char *a) {
    if (strncmp(a, "ok ", 3) == 0) {
        char ip[INET_ADDRSTRLEN];
        const char *bang = strchr(a + 3, '!');
        unsigned port = 0;
        size_t n;
        if (bang == NULL || (n = (size_t)(bang - (a + 3))) >= sizeof ip) {
            errno = EIO;
            return -1;
        }
        memcpy(ip, a + 3, n);
        ip[n] = '\0';
        for (bang++; *bang >= '0' && *bang <= '9'; bang++) {
            port = port * 10 + (unsigned)(*bang - '0');
        }
        memset(&s->local, 0, sizeof s->local);
        s->local.sin_family = AF_INET;
        s->local.sin_port = htons((unsigned short)port);
        (void)inet_pton(AF_INET, ip, &s->local.sin_addr);
        return 0;
    }
    errno = strncmp(a, "fail ", 5) == 0 ? udp_errno(a + 5) : EIO;
    return -1;
}

/* Run netd command `cmd` for `s` and wait for its answer (udp_answer):
 * tagged "#<n>" so the status that carries it is this command's. */
static int udp_ctl(struct myos_sock *s, const char *cmd) {
    char line[80];
    char want[16];
    char path[64];
    struct timeval start;
    size_t n = strlen(cmd);
    size_t w;
    int t;
    want[0] = '#';
    t = myos_u32_dec(want + 1, sizeof want - 2, ++s->ctl_tag);
    if (t < 0 || n + (size_t)t + 2 >= sizeof line
            || conv_path(path, sizeof path, s->proto_path, s->conv, "status") < 0) {
        errno = EINVAL;
        return -1;
    }
    memcpy(line, cmd, n);
    line[n] = ' ';
    memcpy(line + n + 1, want, (size_t)t + 1);
    if (write(s->ctl_fd, line, n + 1 + (size_t)t + 1) < 0) {
        errno = ENOBUFS; /* netfs's request ring is full */
        return -1;
    }
    want[t + 1] = ' ';
    want[t + 2] = '\0';
    w = (size_t)t + 2;
    (void)gettimeofday(&start, NULL);
    for (;;) {
        char st[80];
        ssize_t r;
        int fd = open(path, O_RDONLY);
        if (fd >= 0) {
            r = read(fd, st, sizeof st - 1);
            close(fd);
            if (r > 0) {
                st[r] = '\0';
                if (strncmp(st, want, w) == 0) {
                    return udp_answer(s, st + w);
                }
            }
        }
        if (elapsed_ms(&start) >= UDP_CTL_TIMEOUT_MS) {
            errno = ETIMEDOUT;
            return -1;
        }
        /* netd answers within a round of its loop. */
        {
            struct timespec ms = {0, 1000000};
            (void)nanosleep(&ms, NULL);
        }
    }
}

/* "<verb> a.b.c.d!port" into `cmd`. */
static int udp_endpoint_cmd(char *cmd, size_t cap, const char *verb,
    const struct sockaddr_in *in) {
    char ip[INET_ADDRSTRLEN];
    size_t pos;
    int n;
    if (inet_ntop(AF_INET, &in->sin_addr, ip, sizeof ip) == NULL) {
        return -1;
    }
    n = myos_cpy(cmd, cap, verb);
    if (n < 0) {
        return -1;
    }
    pos = (size_t)n;
    n = myos_cpy(cmd + pos, cap - pos, ip);
    if (n < 0 || pos + (size_t)n + 1 >= cap) {
        return -1;
    }
    pos += (size_t)n;
    cmd[pos++] = '!';
    n = myos_u16_dec(cmd + pos, cap - pos, ntohs(in->sin_port));
    return n < 0 ? -1 : (int)(pos + (size_t)n);
}

static int udp_bind(struct myos_sock *s, const struct sockaddr_in *in) {
    char cmd[48];
    int n = udp_endpoint_cmd(cmd, sizeof cmd - 6, "bind ", in);
    if (n < 0) {
        errno = EINVAL;
        return -1;
    }
    if (s->reuse) {
        memcpy(cmd + n, " reuse", 7);
    }
    return udp_ctl(s, cmd);
}

/* connect, or with AF_UNSPEC undo it: the peer is netd's, the headers go
 * (come back) with it. */
static int udp_connect(struct myos_sock *s, const struct sockaddr *addr, socklen_t addrlen) {
    const struct sockaddr_in *in = (const struct sockaddr_in *)addr;
    char cmd[48];
    if (addr == NULL) {
        errno = EINVAL;
        return -1;
    }
    if (addr->sa_family == AF_UNSPEC) {
        if (udp_ctl(s, "disconnect") < 0) {
            return -1;
        }
        s->peer_set = 0;
        s->state = SOCK_OPEN;
        return s->headers ? 0 : udp_set_headers(s, 1);
    }
    if (addr->sa_family != AF_INET) {
        errno = EAFNOSUPPORT;
        return -1;
    }
    if (addrlen < sizeof *in || udp_endpoint_cmd(cmd, sizeof cmd, "connect ", in) < 0) {
        errno = EINVAL;
        return -1;
    }
    if (udp_ctl(s, cmd) < 0) {
        return -1;
    }
    memset(&s->peer, 0, sizeof s->peer);
    s->peer.sin_family = AF_INET;
    s->peer.sin_port = in->sin_port;
    /* 0.0.0.0 is this host (netd's way, and Linux's). */
    s->peer.sin_addr.s_addr = in->sin_addr.s_addr != INADDR_ANY
        ? in->sin_addr.s_addr : htonl(INADDR_LOOPBACK);
    s->peer_set = 1;
    s->state = SOCK_CONNECTED;
    return s->headers ? udp_set_headers(s, 0) : 0;
}

/* send/sendto/write: `dest` NULL sends to the peer. A connected socket
 * sending elsewhere shows the headers for that one datagram. */
static ssize_t udp_send(struct myos_sock *s, const void *buf, size_t len, int flags,
    const struct sockaddr *dest, socklen_t destlen) {
    unsigned char msg[UDP_HDR + UDP_MAX];
    size_t off = 0;
    long r;
    if (s->shut & SHUT_WR_BIT) {
        if (!(flags & MSG_NOSIGNAL)) {
            raise(SIGPIPE);
        }
        errno = EPIPE;
        return -1;
    }
    if (dest == NULL && !s->peer_set) {
        errno = EDESTADDRREQ;
        return -1;
    }
    if (dest != NULL && dest->sa_family != AF_INET) {
        errno = EAFNOSUPPORT;
        return -1;
    }
    if (dest != NULL && destlen < sizeof(struct sockaddr_in)) {
        errno = EINVAL;
        return -1;
    }
    if (len > UDP_MAX) {
        errno = EMSGSIZE;
        return -1;
    }
    /* The port a first send binds, for getsockname. */
    if (s->local.sin_port == 0 && udp_ctl(s, "autobind") < 0) {
        return -1;
    }
    if (dest != NULL && !s->headers && udp_set_headers(s, 1) < 0) {
        return -1;
    }
    if (s->headers) {
        const struct sockaddr_in *in = (const struct sockaddr_in *)dest;
        memset(msg, 0, UDP_HDR);
        if (in != NULL) {
            memcpy(msg, &in->sin_addr, 4);
            memcpy(msg + 8, &in->sin_port, 2);
        }
        off = UDP_HDR;
    }
    memcpy(msg + off, buf, len);
    r = myos_syscall6(MYOS_SYS_PWRITE, s->data_fd, (long)(uintptr_t)msg, (long)(off + len), 0, 0, 0);
    if (s->peer_set && s->headers) {
        (void)udp_set_headers(s, 0);
    }
    if (r < 0) {
        int e = udp_take_error(s);
        errno = e ? e : ENOBUFS;
        return -1;
    }
    return (ssize_t)len;
}

/* recv/recvfrom/read: the next datagram, cut to `len` (MSG_TRUNC: its
 * whole length returned), from its header or the peer. */
static ssize_t udp_recv(struct myos_sock *s, void *buf, size_t len, int flags,
    struct sockaddr *from, socklen_t *fromlen) {
    unsigned char msg[UDP_HDR + UDP_MAX];
    for (;;) {
        long r;
        if (s->shut & SHUT_RD_BIT) {
            return 0;
        }
        r = myos_syscall6(MYOS_SYS_PREAD, s->data_fd, (long)(uintptr_t)msg, (long)sizeof msg, 0, 0, 0);
        if (r == (long)MYOS_EINTR) {
            errno = EINTR;
            return -1;
        }
        if (r < 0) {
            int e = udp_take_error(s);
            errno = e ? e : EIO;
            return -1;
        }
        if (r > 0 && (!s->headers || r >= UDP_HDR)) {
            struct sockaddr_in src = s->peer;
            const unsigned char *data = msg;
            size_t n = (size_t)r;
            size_t copy;
            if (s->headers) {
                memset(&src, 0, sizeof src);
                src.sin_family = AF_INET;
                memcpy(&src.sin_addr, msg, 4);
                memcpy(&src.sin_port, msg + 8, 2);
                data += UDP_HDR;
                n -= UDP_HDR;
            }
            copy = n < len ? n : len;
            memcpy(buf, data, copy);
            if (from != NULL && fromlen != NULL) {
                socklen_t l = *fromlen < sizeof src ? *fromlen : sizeof src;
                memcpy(from, &src, l);
                *fromlen = sizeof src;
            }
            return (ssize_t)((flags & MSG_TRUNC) ? n : copy);
        }
        if (s->nonblock || (flags & MSG_DONTWAIT)) {
            errno = EAGAIN;
            return -1;
        }
        /* netfs reports POLLIN for a datagram, POLLERR for an error. */
        {
            struct pollfd p = {s->data_fd, POLLIN, 0};
            if (__myos_kpoll(&p, 1, -1) < 0) {
                return -1;
            }
        }
    }
}

ssize_t myos_socket_dgram_read(int fd, void *buf, size_t cnt) {
    if (!is_dgram(fd)) {
        return -2;
    }
    return udp_recv(sock_by_fd(fd), buf, cnt, 0, NULL, NULL);
}

ssize_t myos_socket_dgram_write(int fd, const void *buf, size_t cnt) {
    if (!is_dgram(fd)) {
        return -2;
    }
    return udp_send(sock_by_fd(fd), buf, cnt, 0, NULL, 0);
}
