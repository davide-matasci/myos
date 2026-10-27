/*
 * Userspace BSD sockets over Plan 9 /net + netd (smoltcp).
 * No socket() syscall — TCP and UDP via clone/ctl/data, and AF_UNIX
 * stream and record (SOCK_SEQPACKET) sockets over /net/unix, which the
 * kernel serves itself (docs/sockets-unix.md).
 *
 * AF_INET and AF_INET6 are the same conversations: netd names an address
 * in its text form either way (`a.b.c.d!port`, `fec0::2!port`). An
 * AF_INET6 socket on `::` takes IPv4 peers too (unless IPV6_V6ONLY), which
 * it names `::ffff:a.b.c.d`, as on Linux; one connected or sending to such
 * an address talks IPv4 to netd.
 *
 * A UDP socket keeps its ctl open, and an fd of the conversation's hdata
 * (close-on-exec): there each datagram read or written carries netd's
 * 36-byte header (remote address, local address: 16 bytes each, IPv4 as
 * ::ffff:a.b.c.d; remote port, local port: Plan 9's udp headers without
 * the interface address), which recvfrom and sendto turn into addresses. The
 * socket's own fd, its data, has bare datagrams (to the peer when written),
 * which is all a program it is passed to by exec sees until it uses it as a
 * socket (sock_adopt). Binding, connecting and their undoing are netd's
 * (udp_ctl); an error netd reports (a refused datagram) fails the next read
 * or write and is SO_ERROR. A connect drops the datagrams already queued
 * from anyone else.
 *
 * The table of sockets is the process's: one exec'd with a socket fd finds
 * it on its first socket call by the fd's name (/proc/self/fd/N), and the
 * conversation's state (sock_adopt).
 */
#include <errno.h>
#include <fcntl.h>
#include "myos_fmt.h"
#include <poll.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

#include <arpa/inet.h>
#include <ifaddrs.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <sys/un.h>

#include "libmyos.h"
#include "libc.h"

/* As many sockets as fds: the fd limit is the only one (an X server takes a
 * socket per client). */
#define MYOS_MAX_SOCKS MYOS_OPEN_MAX
#define UN_NAME_CAP ((int)sizeof(((struct sockaddr_un *)0)->sun_path))
/* The largest SOCK_SEQPACKET record: what the kernel hands a file in one
 * write (a larger write reaches /net/unix in pieces, each a record of its
 * own; netfs's RECORD_MAX). A larger one is EMSGSIZE. */
#define UN_RECORD_MAX (8 * 1024)
#define CONNECT_TIMEOUT_MS 30000
/* A UDP datagram's header to and from netd, and the most one netfs
 * message carries behind it. */
#define UDP_HDR 36
#define UDP_MAX 2006
/* netd's text of an endpoint: an IPv6 address, '!', a port. */
#define EP_TEXT (INET6_ADDRSTRLEN + 6)
/* A status line of netd's: an accept's or a UDP ctl answer's, two
 * endpoints. */
#define STATUS_LINE 160
/* How long netd may take to answer a UDP ctl command, or a listen. */
#define UDP_CTL_TIMEOUT_MS 5000

enum {
    SOCK_UNUSED = 0,
    SOCK_OPEN,
    SOCK_CONNECTING, /* nonblock connect in flight; wait for Established */
    SOCK_CONNECTED,
    SOCK_LISTENING,  /* announced (bound + listening); __myos_accept() incoming */
};

/* An IP endpoint as netd has it: IPv4 (the address in the first four bytes
 * of addr) or IPv6, the port in network order. */
struct ep {
    int v6;
    unsigned char addr[16];
    unsigned short port;
};

struct myos_sock {
    int used;
    int state;
    int type;       /* SOCK_STREAM / SOCK_DGRAM / SOCK_SEQPACKET (AF_UNIX) */
    int data_fd;    /* returned to the app; /net/.../data */
    int ctl_fd;     /* kept open through connect */
    int nonblock;   /* O_NONBLOCK via fcntl F_SETFL (userspace-tracked) */
    int so_error;   /* pending SO_ERROR (connect failure); cleared on read */
    unsigned short conv;
    char proto_path[16]; /* "/net/tcp" or "/net/udp" */
    struct ep peer;
    int peer_set;
    struct ep bound; /* TCP: the address and port __myos_bind() named */
    int accept_armed;  /* listener: "accept" ctl written, waiting for status */
    int last_accept_seq; /* listener: seq of the last accepted handoff */
    int taken_seq;     /* listener: seq named by the last "taken <seq>" sent */
    struct timeval taken_tv; /* listener: when that "taken" was sent */
    int family;     /* AF_INET, AF_INET6 or AF_UNIX */
    int v6only;     /* AF_INET6: IPV6_V6ONLY, no IPv4 peers */
    char un_name[UN_NAME_CAP + 1]; /* AF_UNIX: bound (listener's) name */
    char un_peer[UN_NAME_CAP + 1]; /* AF_UNIX: the name connected to */
    /* UDP: */
    int hdata_fd;   /* /net/udp/N/hdata: datagrams with their headers */
    struct ep local; /* local address and port, netd's answers */
    int shut;       /* __myos_shutdown(): SHUT_RD_BIT, SHUT_WR_BIT */
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

static int udp_bind(struct myos_sock *s, const struct ep *ep);
static int udp_connect(struct myos_sock *s, const struct sockaddr *addr, socklen_t addrlen);
static int udp_take_error(struct myos_sock *s);
static int udp_open_hdata(struct myos_sock *s);
static int udp_ctl(struct myos_sock *s, const char *cmd);
static struct myos_sock *sock_adopt(int fd);
static ssize_t udp_send(struct myos_sock *s, const void *buf, size_t len, int flags,
    const struct sockaddr *dest, socklen_t destlen);
static ssize_t udp_recv(struct myos_sock *s, void *buf, size_t len, int flags,
    struct sockaddr *from, socklen_t *fromlen);

static const unsigned char v4mapped[12] = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff};

static int ep_unspecified(const struct ep *ep) {
    static const unsigned char zero[16];
    return memcmp(ep->addr, zero, ep->v6 ? 16 : 4) == 0;
}

/* The loopback address of `ep`'s version (an unspecified address is this
 * host, netd's way and Linux's). */
static void ep_localhost(struct ep *ep) {
    memset(ep->addr, 0, sizeof ep->addr);
    if (ep->v6) {
        ep->addr[15] = 1;
    } else {
        ep->addr[0] = 127;
        ep->addr[3] = 1;
    }
}

/* `addr` (`len` bytes) as an endpoint of `s`'s family: an AF_INET socket
 * takes a sockaddr_in, an AF_INET6 one a sockaddr_in6, whose IPv4-mapped
 * address is the IPv4 one (which a v6only socket cannot reach). 0, or -1
 * with errno set. */
static int ep_from(const struct myos_sock *s, const struct sockaddr *addr, socklen_t len, struct ep *ep) {
    memset(ep, 0, sizeof *ep);
    if (addr == NULL || addr->sa_family != s->family) {
        errno = EAFNOSUPPORT;
        return -1;
    }
    if (s->family == AF_INET) {
        const struct sockaddr_in *in = (const struct sockaddr_in *)addr;
        if (len < sizeof *in) {
            errno = EINVAL;
            return -1;
        }
        memcpy(ep->addr, &in->sin_addr, 4);
        ep->port = in->sin_port;
        return 0;
    }
    {
        const struct sockaddr_in6 *in6 = (const struct sockaddr_in6 *)addr;
        if (len < sizeof *in6) {
            errno = EINVAL;
            return -1;
        }
        ep->port = in6->sin6_port;
        if (memcmp(in6->sin6_addr.s6_addr, v4mapped, 12) == 0) {
            if (s->v6only) {
                errno = ENETUNREACH;
                return -1;
            }
            memcpy(ep->addr, in6->sin6_addr.s6_addr + 12, 4);
            return 0;
        }
        ep->v6 = 1;
        memcpy(ep->addr, in6->sin6_addr.s6_addr, 16);
        return 0;
    }
}

/* `ep` as a sockaddr of `s`'s family into `addr` (`*len` bytes of room),
 * `*len` its whole size. An AF_INET6 socket names an IPv4 address
 * ::ffff:a.b.c.d (0.0.0.0 is ::). */
static void ep_put(const struct myos_sock *s, const struct ep *ep, struct sockaddr *addr, socklen_t *len) {
    union {
        struct sockaddr_in in;
        struct sockaddr_in6 in6;
    } u;
    socklen_t n;
    if (addr == NULL || len == NULL) {
        return;
    }
    memset(&u, 0, sizeof u);
    if (s->family == AF_INET6) {
        u.in6.sin6_family = AF_INET6;
        u.in6.sin6_port = ep->port;
        if (ep->v6) {
            memcpy(u.in6.sin6_addr.s6_addr, ep->addr, 16);
        } else if (!ep_unspecified(ep)) {
            memcpy(u.in6.sin6_addr.s6_addr, v4mapped, 12);
            memcpy(u.in6.sin6_addr.s6_addr + 12, ep->addr, 4);
        }
        n = sizeof u.in6;
    } else {
        u.in.sin_family = AF_INET;
        u.in.sin_port = ep->port;
        if (!ep->v6) {
            memcpy(&u.in.sin_addr, ep->addr, 4);
        }
        n = sizeof u.in;
    }
    memcpy(addr, &u, *len < n ? *len : n);
    *len = n;
}

/* The any address of `s`'s family, port 0. */
static void ep_any(const struct myos_sock *s, struct ep *ep) {
    memset(ep, 0, sizeof *ep);
    ep->v6 = s->family == AF_INET6;
}

/* "<verb>addr!port" into `cmd`; its length, or -1. */
static int ep_cmd(char *cmd, size_t cap, const char *verb, const struct ep *ep) {
    char ip[INET6_ADDRSTRLEN];
    size_t pos;
    int n;
    if (inet_ntop(ep->v6 ? AF_INET6 : AF_INET, ep->addr, ip, sizeof ip) == NULL) {
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
    n = myos_u16_dec(cmd + pos, cap - pos, ntohs(ep->port));
    return n < 0 ? -1 : (int)(pos + (size_t)n);
}

/* netd's "addr!port" at `t` into `ep`; the text after it, or NULL. */
static const char *ep_parse(const char *t, struct ep *ep) {
    char ip[INET6_ADDRSTRLEN];
    const char *bang = strchr(t, '!');
    unsigned port = 0;
    size_t n;
    if (bang == NULL || (n = (size_t)(bang - t)) >= sizeof ip) {
        return NULL;
    }
    memcpy(ip, t, n);
    ip[n] = '\0';
    memset(ep, 0, sizeof *ep);
    if (inet_pton(AF_INET, ip, ep->addr) != 1) {
        if (inet_pton(AF_INET6, ip, ep->addr) != 1) {
            return NULL;
        }
        ep->v6 = 1;
    }
    for (bang++; *bang >= '0' && *bang <= '9'; bang++) {
        port = port * 10 + (unsigned)(*bang - '0');
    }
    ep->port = htons((unsigned short)port);
    return bang;
}

/* `ep`'s address as in netd's datagram header (16 bytes, IPv4 as
 * ::ffff:a.b.c.d), and back. */
static void ep_to_hdr(const struct ep *ep, unsigned char *out) {
    if (ep->v6) {
        memcpy(out, ep->addr, 16);
    } else {
        memcpy(out, v4mapped, 12);
        memcpy(out + 12, ep->addr, 4);
    }
}

static void ep_from_hdr(const unsigned char *in, struct ep *ep) {
    memset(ep, 0, sizeof *ep);
    if (memcmp(in, v4mapped, 12) == 0) {
        memcpy(ep->addr, in + 12, 4);
    } else {
        ep->v6 = 1;
        memcpy(ep->addr, in, 16);
    }
}

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

/* The socket of `fd` for a socket call: one of the table, or one this
 * process inherited by exec (sock_adopt). */
static struct myos_sock *sock_lookup(int fd) {
    struct myos_sock *s = sock_by_fd(fd);
    return s != NULL ? s : sock_adopt(fd);
}

static struct myos_sock *sock_alloc(void) {
    int i;
    for (i = 0; i < MYOS_MAX_SOCKS; i++) {
        if (!socks[i].used) {
            memset(&socks[i], 0, sizeof(socks[i]));
            socks[i].used = 1;
            socks[i].data_fd = -1;
            socks[i].ctl_fd = -1;
            socks[i].hdata_fd = -1;
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
    s->hdata_fd = -1;
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

static int parse_clone_id(const char *buf, size_t n, unsigned short *out);

/* The number of the conversation of `proto_path` (/net/tcp) whose data
 * `fd` is open on, from its /proc/self/fd link: 0, or -1 when it is none. */
static int fd_conv(int fd, const char *proto_path, unsigned short *id) {
    char link[32] = "/proc/self/fd/";
    char path[64];
    size_t plen = strlen(proto_path);
    const char *rest;
    ssize_t n;
    if (fd < 0 || myos_u16_dec(link + 14, sizeof link - 14, (unsigned)fd) < 0) {
        return -1;
    }
    n = readlink(link, path, sizeof path - 1);
    if (n <= 0) {
        return -1;
    }
    path[n] = '\0';
    if (strncmp(path, proto_path, plen) != 0 || path[plen] != '/') {
        return -1;
    }
    rest = path + plen + 1;
    if (parse_clone_id(rest, strcspn(rest, "/"), id) < 0 || strcmp(rest + strcspn(rest, "/"), "/data") != 0) {
        return -1;
    }
    return 0;
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
 * curl/LibreSSL treat read/recv 0 as peer close (SSL EOF). */
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
 * full, or netd has a TCP conversation's 8 KiB of sends still queued; a
 * unix record socket when the whole record does not fit.
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
    if (s == NULL || (s->type != SOCK_STREAM && s->type != SOCK_SEQPACKET)) {
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

/* Whether a write of `cnt` bytes to `fd` is a record larger than a record
 * socket takes (UN_RECORD_MAX): a socket of the table (a write is no
 * socket call that adopts one). */
int myos_socket_record_too_big(int fd, size_t cnt) {
    struct myos_sock *s;
    if (cnt <= UN_RECORD_MAX) {
        return 0;
    }
    s = sock_by_fd(fd);
    return s != NULL && s->type == SOCK_SEQPACKET;
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
static int netd_errno(const char *why);
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
        if (s->type != SOCK_DGRAM) {
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
    if (s->hdata_fd >= 0) {
        close(s->hdata_fd);
    }
    if (s->ctl_fd >= 0) {
        close(s->ctl_fd);
    }
    s->data_fd = -1;
    sock_free(s);
}

int __myos_socket(int domain, int type, int protocol) {
    struct myos_sock *s;
    char clone_path[32];
    char path[64];
    int data_fd;
    int ctl_fd;
    unsigned short id;
    const char *proto;

    (void)protocol;

    if (domain == AF_UNIX) {
        if (type != SOCK_STREAM && type != SOCK_SEQPACKET) {
            errno = EPROTONOSUPPORT;
            return -1;
        }
        proto = "/net/unix";
    } else if (domain != AF_INET && domain != AF_INET6) {
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
    /* Opening clone makes the conversation, and the fd is on its data: the
     * socket. Its last close hangs the conversation up. */
    data_fd = open(clone_path, O_RDWR);
    if (data_fd < 0) {
        sock_free(s);
        errno = EIO;
        return -1;
    }
    if (fd_conv(data_fd, proto, &id) < 0 || conv_path(path, sizeof path, proto, id, "ctl") < 0) {
        close(data_fd);
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
        close(data_fd);
        sock_free(s);
        errno = EIO;
        return -1;
    }

    /* A record socket's conversation keeps records from the start. */
    if (type == SOCK_SEQPACKET && write(ctl_fd, "seqpacket", 9) < 0) {
        close(ctl_fd);
        close(data_fd);
        sock_free(s);
        errno = EIO;
        return -1;
    }
    strncpy(s->proto_path, proto, sizeof s->proto_path - 1);
    s->proto_path[sizeof s->proto_path - 1] = '\0';
    s->conv = id;
    /* An exec'd program opens its own (sock_adopt). */
    (void)fcntl(ctl_fd, F_SETFD, FD_CLOEXEC);
    s->ctl_fd = ctl_fd;
    s->data_fd = data_fd;
    s->type = type;
    s->family = domain;
    s->state = SOCK_OPEN;
    ep_any(s, &s->bound);
    ep_any(s, &s->local);
    if (type == SOCK_DGRAM) {
        if (data_fd >= MYOS_MAX_SOCKS || udp_open_hdata(s) < 0) {
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

/* What a TCP socket may bind, as on Linux: any address, 127.0.0.0/8, ::1
 * and the host's own (/net/ifaddrs). */
static int host_addr(const struct ep *a) {
    static const unsigned char lo6[16] = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1};
    struct ifaddrs *ifs, *p;
    int found = 0;
    if (ep_unspecified(a) || (!a->v6 && a->addr[0] == 127) || (a->v6 && memcmp(a->addr, lo6, 16) == 0)) {
        return 1;
    }
    if (getifaddrs(&ifs) != 0) {
        return 0;
    }
    for (p = ifs; p != NULL; p = p->ifa_next) {
        if (p->ifa_addr == NULL) {
            continue;
        }
        if (!a->v6 && p->ifa_addr->sa_family == AF_INET
            && memcmp(&((struct sockaddr_in *)p->ifa_addr)->sin_addr, a->addr, 4) == 0) {
            found = 1;
        }
        if (a->v6 && p->ifa_addr->sa_family == AF_INET6
            && memcmp(((struct sockaddr_in6 *)p->ifa_addr)->sin6_addr.s6_addr, a->addr, 16) == 0) {
            found = 1;
        }
    }
    freeifaddrs(ifs);
    return found;
}

int __myos_bind(int sockfd, const struct sockaddr *addr, socklen_t addrlen) {
    struct myos_sock *s = sock_lookup(sockfd);
    if (s == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    if (addr != NULL && s->family == AF_UNIX) {
        /* The name is announced by __myos_listen(). */
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
    {
        struct ep ep;
        if (addr == NULL) {
            errno = EINVAL;
            return -1;
        }
        if (ep_from(s, addr, addrlen, &ep) < 0) {
            /* An IPv4-mapped address a v6only socket cannot have. */
            if (errno == ENETUNREACH) {
                errno = EADDRNOTAVAIL;
            }
            return -1;
        }
        if (s->type == SOCK_DGRAM) {
            return udp_bind(s, &ep);
        }
        if (!host_addr(&ep)) {
            errno = EADDRNOTAVAIL;
            return -1;
        }
        /* Remembered; the listener is started by __myos_listen(). */
        s->bound = ep;
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
    char sbuf[STATUS_LINE];
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

/* AF_UNIX: open conversation <id> (connected) as a socket fd of `type`. */
static int un_open(unsigned short id, const char *name, int type) {
    char path[64];
    struct myos_sock *s = sock_alloc();
    if (s == NULL) {
        errno = EMFILE;
        return -1;
    }
    strcpy(s->proto_path, "/net/unix");
    s->conv = id;
    s->type = type;
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
    fd = un_open(id, ls->un_name, ls->type);
    if (fd >= 0) {
        un_put_name("", addr, addrlen);
    }
    return fd;
}

int __myos_socketpair(int domain, int type, int protocol, int sv[2]) {
    struct myos_sock *s;
    unsigned short id;
    int a;
    int b;
    if (domain != AF_UNIX) {
        errno = EAFNOSUPPORT;
        return -1;
    }
    if (type != SOCK_STREAM && type != SOCK_SEQPACKET) {
        errno = EPROTONOSUPPORT;
        return -1;
    }
    a = __myos_socket(AF_UNIX, type, protocol);
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
    b = un_open(id, "", type);
    if (b < 0) {
        close(a);
        return -1;
    }
    sv[0] = a;
    sv[1] = b;
    return 0;
}

int __myos_listen(int sockfd, int backlog) {
    struct myos_sock *s = sock_lookup(sockfd);
    char cmd[sizeof "announce " + EP_TEXT + sizeof " v6only"];
    (void)backlog; /* netd keeps a one-connection ready queue (smoltcp listener) */
    if (s == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    if (s->type != SOCK_STREAM && s->type != SOCK_SEQPACKET) {
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
    if (s->state == SOCK_LISTENING) {
        return 0; /* already listening: Linux takes the new backlog */
    }
    {
        /* "announce addr!port": netd listens on that address (0.0.0.0: any
         * IPv4 one; ::, any IPv6 one and IPv4 too unless v6only) and port;
         * port 0 (a bind to it, or none) is a free one, named in netd's
         * answer ("listening <port>"), which getsockname then reports. */
        int n = ep_cmd(cmd, sizeof cmd - sizeof " v6only", "announce ", &s->bound);
        if (n < 0) {
            errno = EINVAL;
            return -1;
        }
        if (s->bound.v6 && s->v6only) {
            memcpy(cmd + n, " v6only", sizeof " v6only");
        }
    }
    if (listener_ctl(s, cmd) < 0) {
        return -1;
    }
    {
        /* netd answers "listening" (a first connection may already be
         * "accepted"), or why not, in place of the conv's "cloned"
         * ("addrinuse": another listener has the port). */
        struct timeval start;
        char st[STATUS_LINE];
        (void)gettimeofday(&start, NULL);
        for (;;) {
            if (listener_status(s, st, sizeof st) == 0 && strcmp(st, "cloned") != 0) {
                if (strncmp(st, "listening", 9) == 0 || strncmp(st, "accepted", 8) == 0) {
                    if (s->bound.port == 0) {
                        long port = strncmp(st, "listening ", 10) == 0 ? strtol(st + 10, NULL, 10) : 0;
                        if (port <= 0 || port > 65535) {
                            errno = EADDRINUSE;
                            return -1;
                        }
                        s->bound.port = htons((unsigned short)port);
                    }
                    break;
                }
                errno = netd_errno(st);
                return -1;
            }
            if (elapsed_ms(&start) >= UDP_CTL_TIMEOUT_MS) {
                errno = ETIMEDOUT;
                return -1;
            }
            {
                struct timespec ms = {0, 1000000};
                (void)nanosleep(&ms, NULL);
            }
        }
    }
    s->state = SOCK_LISTENING;
    return 0;
}

static int accept_body(int sockfd, struct sockaddr *addr, socklen_t *addrlen) {
    struct myos_sock *ls = sock_lookup(sockfd);
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
        char stbuf[STATUS_LINE];
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
        char stbuf[STATUS_LINE];
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
    s->family = ls->family;
    s->v6only = ls->v6only;
    s->state = SOCK_CONNECTED;
    s->bound = ls->bound;
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
     * bump seq), so the next __myos_accept() re-opened the live Child — dropbear
     * Integrity error bad packet size 0x53534831 on sequential SSH. */
    listener_send_taken(ls, (int)seq);
    /* Optional peer from "accepted <N> <ip>!<port>" — best effort. */
    while (*p == ' ') {
        p++;
    }
    if (*p != '\0' && ep_parse(p, &s->peer) != NULL) {
        s->peer_set = 1;
        ep_put(s, &s->peer, addr, addrlen);
    }
    return data_fd;
}

static int connect_body(int sockfd, const struct sockaddr *addr, socklen_t addrlen) {
    struct myos_sock *s = sock_lookup(sockfd);
    struct ep ep;
    char cmd[sizeof "connect " + EP_TEXT];
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
    if (ep_from(s, addr, addrlen, &ep) < 0) {
        return -1;
    }
    if ((cm = ep_cmd(cmd, sizeof cmd, "connect ", &ep)) < 0) {
        errno = EINVAL;
        return -1;
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
    s->peer = ep;
    if (ep_unspecified(&s->peer)) {
        ep_localhost(&s->peer);
    }
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

int __myos_shutdown(int sockfd, int how) {
    struct myos_sock *s = sock_lookup(sockfd);
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

int __myos_setsockopt(int sockfd, int level, int optname, const void *optval, socklen_t optlen) {
    struct myos_sock *s = sock_lookup(sockfd);
    if (s == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    /* Kept for a UDP bind (SO_REUSEADDR) and getsockopt; the others are
     * accepted and ignored so curl/LibreSSL keep going. */
    if (level == SOL_SOCKET && (optname == SO_REUSEADDR || optname == SO_BROADCAST)) {
        if (optval == NULL || optlen < sizeof(int)) {
            errno = EINVAL;
            return -1;
        }
        *(optname == SO_REUSEADDR ? &s->reuse : &s->broadcast) = *(const int *)optval != 0;
    }
    /* Taken at the bind or listen, as on Linux (where a bound socket
     * refuses it). */
    if (level == IPPROTO_IPV6 && optname == IPV6_V6ONLY) {
        if (s->family != AF_INET6) {
            errno = ENOPROTOOPT;
            return -1;
        }
        if (optval == NULL || optlen < sizeof(int)) {
            errno = EINVAL;
            return -1;
        }
        s->v6only = *(const int *)optval != 0;
    }
    return 0;
}

int __myos_getsockopt(int sockfd, int level, int optname, void *optval, socklen_t *optlen) {
    struct myos_sock *s = sock_lookup(sockfd);
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
    if (level == IPPROTO_IPV6 && optname == IPV6_V6ONLY && s->family == AF_INET6) {
        if (*optlen < sizeof(int)) {
            errno = EINVAL;
            return -1;
        }
        *(int *)optval = s->v6only;
        *optlen = sizeof(int);
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

int __myos_getsockname(int sockfd, struct sockaddr *addr, socklen_t *addrlen) {
    struct myos_sock *s = sock_lookup(sockfd);
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
    /* A UDP socket's is netd's; a TCP one's what __myos_bind() named (a
     * connected one's is not known: any). */
    ep_put(s, s->type == SOCK_DGRAM ? &s->local : &s->bound, addr, addrlen);
    return 0;
}

int __myos_getpeername(int sockfd, struct sockaddr *addr, socklen_t *addrlen) {
    struct myos_sock *s = sock_lookup(sockfd);
    if (s == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    /* A UDP socket connected to port 0 has no peer, as on Linux. */
    if (!s->peer_set || (s->type == SOCK_DGRAM && s->peer.port == 0)) {
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
    ep_put(s, &s->peer, addr, addrlen);
    return 0;
}

ssize_t __myos_send(int sockfd, const void *buf, size_t len, int flags) {
    struct myos_sock *s = sock_lookup(sockfd);
    if (is_dgram(sockfd)) {
        return udp_send(s, buf, len, flags, NULL, 0);
    }
    (void)flags;
    if (s == NULL) {
        /* Allow plain write path if somehow untracked; still try write. */
    }
    return write(sockfd, buf, len);
}

/* MSG_PEEK on a stream: what the conversation's `peek` gives, which netfs
 * leaves for `data`; waiting for it as a read of `data` would. */
static ssize_t tcp_peek(struct myos_sock *s, void *buf, size_t len, int flags) {
    char path[64];
    ssize_t n;
    int fd;
    if (conv_path(path, sizeof path, s->proto_path, s->conv, "peek") < 0) {
        errno = EIO;
        return -1;
    }
    for (;;) {
        fd = open(path, O_RDONLY | O_CLOEXEC);
        if (fd < 0) {
            errno = ENOTCONN;
            return -1;
        }
        n = read(fd, buf, len);
        close(fd);
        if (n > 0 || len == 0) {
            return n;
        }
        switch (rx_state(s)) {
        case 0:
            continue;
        case 1:
            return 0;
        }
        if (s->nonblock || (flags & MSG_DONTWAIT)) {
            errno = EAGAIN;
            return -1;
        }
        if (wait_readable(s) < 0) {
            errno = EINTR;
            return -1;
        }
    }
}

ssize_t __myos_recv(int sockfd, void *buf, size_t len, int flags) {
    ssize_t n;
    struct myos_sock *s = sock_lookup(sockfd);
    int restore = 0;
    if (is_dgram(sockfd)) {
        return udp_recv(s, buf, len, flags, NULL, NULL);
    }
    if (s != NULL && (flags & MSG_PEEK) && s->family != AF_UNIX && s->state == SOCK_CONNECTED) {
        return tcp_peek(s, buf, len, flags);
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

/* The fds of msg's SCM_RIGHTS messages into `fds` (at most `cap`): how
 * many, or -1 when the control data is malformed. */
static int sent_rights(const struct msghdr *msg, int *fds, int cap) {
    struct msghdr m = *msg;
    struct cmsghdr *c;
    int n = 0;
    if (m.msg_control == NULL) {
        return 0;
    }
    for (c = CMSG_FIRSTHDR(&m); c != NULL; c = CMSG_NXTHDR(&m, c)) {
        size_t k;
        if (c->cmsg_len < CMSG_LEN(0)) {
            return -1;
        }
        if (c->cmsg_level != SOL_SOCKET || c->cmsg_type != SCM_RIGHTS) {
            continue;
        }
        for (k = 0; k < (c->cmsg_len - CMSG_LEN(0)) / sizeof(int); k++) {
            if (n == cap) {
                return -1;
            }
            memcpy(&fds[n++], CMSG_DATA(c) + k * sizeof(int), sizeof(int));
        }
    }
    return n;
}

/* sendmsg: the data, with the fds of an SCM_RIGHTS message on a unix
 * socket: the conversation's "rights" takes them and they come with the
 * first byte (docs/sockets-unix.md). */
ssize_t __myos_sendmsg(int sockfd, const struct msghdr *msg, int flags) {
    struct myos_sock *s = sock_lookup(sockfd);
    int fds[253];
    int n = sent_rights(msg, fds, 253);
    ssize_t r;
    if (n < 0) {
        errno = EINVAL;
        return -1;
    }
    /* One record: writev gathers it into one write up to its own limit,
     * which is the largest record. */
    if (s != NULL && s->type == SOCK_SEQPACKET) {
        size_t total = 0, i;
        for (i = 0; i < msg->msg_iovlen; i++) {
            total += msg->msg_iov[i].iov_len;
        }
        if (total > UN_RECORD_MAX) {
            errno = EMSGSIZE;
            return -1;
        }
    }
    if (n > 0) {
        char cmd[sizeof "rights" + 253 * 4];
        size_t pos = 6;
        int i;
        if (s == NULL || s->family != AF_UNIX) {
            errno = s == NULL ? ENOTSOCK : EINVAL;
            return -1;
        }
        memcpy(cmd, "rights", 6);
        for (i = 0; i < n; i++) {
            int w;
            if (fds[i] < 0) {
                errno = EBADF;
                return -1;
            }
            cmd[pos++] = ' ';
            w = myos_u16_dec(cmd + pos, sizeof cmd - pos, (unsigned short)fds[i]);
            if (w < 0) {
                errno = EBADF;
                return -1;
            }
            pos += (size_t)w;
        }
        cmd[pos] = '\0';
        if (listener_ctl(s, cmd) < 0) {
            errno = EBADF; /* an fd that is not open */
            return -1;
        }
    }
    if (msg->msg_name != NULL && s != NULL && s->state != SOCK_CONNECTED && !is_dgram(sockfd)) {
        if (__myos_connect(sockfd, msg->msg_name, msg->msg_namelen) < 0) {
            return -1;
        }
    }
    if (is_dgram(sockfd)) {
        /* One datagram: the iovecs gathered (no fds on UDP). */
        size_t total = 0, off = 0, i;
        char *buf;
        for (i = 0; i < msg->msg_iovlen; i++) {
            total += msg->msg_iov[i].iov_len;
        }
        buf = malloc(total ? total : 1);
        if (buf == NULL) {
            errno = ENOMEM;
            return -1;
        }
        for (i = 0; i < msg->msg_iovlen; i++) {
            memcpy(buf + off, msg->msg_iov[i].iov_base, msg->msg_iov[i].iov_len);
            off += msg->msg_iov[i].iov_len;
        }
        r = udp_send(s, buf, total, flags, msg->msg_name, msg->msg_namelen);
        free(buf);
        return r;
    }
    r = writev(sockfd, msg->msg_iov, (int)msg->msg_iovlen);
    if (n > 0 && r <= 0) {
        int e = errno;
        /* Not sent: a retry sends them again. */
        listener_ctl(s, "norights");
        errno = e;
    }
    return r;
}

/* The fds that came with what the last read of `s` took: the
 * conversation's "rights" makes them this process's. How many, into `fds`. */
static int received_rights(struct myos_sock *s, int *fds, int cap) {
    char path[64];
    char buf[253 * 4 + 2];
    int fd, n = 0;
    ssize_t len, i;
    if (conv_path(path, sizeof path, s->proto_path, s->conv, "rights") < 0) {
        return 0;
    }
    fd = open(path, O_RDONLY);
    if (fd < 0) {
        return 0;
    }
    len = read(fd, buf, sizeof buf - 1);
    close(fd);
    for (i = 0; i < len; ) {
        int v = 0, any = 0;
        while (i < len && buf[i] >= '0' && buf[i] <= '9') {
            v = v * 10 + (buf[i++] - '0');
            any = 1;
        }
        if (any && n < cap) {
            fds[n++] = v;
        }
        i++;
    }
    return n;
}

/* Whether the last read of record socket `s` dropped the rest of a record:
 * its conversation's ctl says "seqpacket cut". */
static int un_record_cut(struct myos_sock *s) {
    char path[64];
    char buf[32];
    ssize_t n;
    int fd;
    if (conv_path(path, sizeof path, s->proto_path, s->conv, "ctl") < 0
        || (fd = open(path, O_RDONLY)) < 0) {
        return 0;
    }
    n = read(fd, buf, sizeof buf);
    close(fd);
    return n > 0 && buf_has(buf, (size_t)n, "cut");
}

/* recvmsg: the data, and on a unix socket the fds that came with it as an
 * SCM_RIGHTS message; those msg_control has no room for are closed
 * (MSG_CTRUNC). */
ssize_t __myos_recvmsg(int sockfd, struct msghdr *msg, int flags) {
    struct myos_sock *s = sock_lookup(sockfd);
    int restore = 0, fds[253], n = 0, kept, i;
    ssize_t r;
    if (is_dgram(sockfd)) {
        /* One datagram, scattered over the iovecs. */
        size_t total = 0, off = 0, k;
        char *buf;
        for (k = 0; k < msg->msg_iovlen; k++) {
            total += msg->msg_iov[k].iov_len;
        }
        buf = malloc(total ? total : 1);
        if (buf == NULL) {
            errno = ENOMEM;
            return -1;
        }
        r = udp_recv(s, buf, total, flags, msg->msg_name, &msg->msg_namelen);
        for (k = 0; r > 0 && k < msg->msg_iovlen && off < (size_t)r; k++) {
            size_t take = msg->msg_iov[k].iov_len < (size_t)r - off ? msg->msg_iov[k].iov_len : (size_t)r - off;
            memcpy(msg->msg_iov[k].iov_base, buf + off, take);
            off += take;
        }
        free(buf);
        msg->msg_controllen = 0;
        msg->msg_flags = 0;
        return r;
    }
    if (s != NULL && (flags & MSG_DONTWAIT) && !s->nonblock) {
        s->nonblock = 1;
        restore = 1;
    }
    r = readv(sockfd, msg->msg_iov, (int)msg->msg_iovlen);
    if (restore) {
        s->nonblock = 0;
    }
    msg->msg_flags = 0;
    msg->msg_namelen = 0;
    if (r > 0 && s != NULL && s->family == AF_UNIX) {
        n = received_rights(s, fds, 253);
    }
    /* A record that filled the buffers may have been longer. */
    if (r > 0 && s != NULL && s->type == SOCK_SEQPACKET && un_record_cut(s)) {
        msg->msg_flags |= MSG_TRUNC;
    }
    kept = 0;
    if (n > 0 && msg->msg_control != NULL && msg->msg_controllen >= CMSG_LEN(sizeof(int))) {
        kept = (int)((msg->msg_controllen - CMSG_LEN(0)) / sizeof(int));
        if (kept > n) {
            kept = n;
        }
    }
    for (i = kept; i < n; i++) {
        close(fds[i]);
    }
    if (kept < n) {
        msg->msg_flags |= MSG_CTRUNC;
    }
    for (i = 0; i < kept; i++) {
        if (flags & MSG_CMSG_CLOEXEC) {
            fcntl(fds[i], F_SETFD, FD_CLOEXEC);
        }
    }
    if (kept > 0) {
        struct cmsghdr *c = (struct cmsghdr *)msg->msg_control;
        size_t space = CMSG_SPACE(kept * sizeof(int));
        c->cmsg_len = CMSG_LEN(kept * sizeof(int));
        c->cmsg_level = SOL_SOCKET;
        c->cmsg_type = SCM_RIGHTS;
        memcpy(CMSG_DATA(c), fds, kept * sizeof(int));
        msg->msg_controllen = space < msg->msg_controllen ? space : msg->msg_controllen;
    } else {
        msg->msg_controllen = 0;
    }
    return r;
}

ssize_t __myos_sendto(int sockfd, const void *buf, size_t len, int flags,
    const struct sockaddr *dest_addr, socklen_t addrlen) {
    struct myos_sock *s = sock_lookup(sockfd);
    if (is_dgram(sockfd)) {
        return udp_send(s, buf, len, flags, dest_addr, addrlen);
    }
    (void)flags;
    if (s != NULL && s->state != SOCK_CONNECTED && dest_addr != NULL) {
        if (__myos_connect(sockfd, dest_addr, addrlen) < 0) {
            return -1;
        }
    }
    return write(sockfd, buf, len);
}

ssize_t __myos_recvfrom(int sockfd, void *buf, size_t len, int flags,
    struct sockaddr *src_addr, socklen_t *addrlen) {
    ssize_t n;
    struct myos_sock *s;
    int restore = 0;
    s = sock_lookup(sockfd);
    if (is_dgram(sockfd)) {
        return udp_recv(s, buf, len, flags, src_addr, addrlen);
    }
    /* As __myos_recv: a peek leaves the bytes (mlibc's recv is a recvfrom
     * without an address). */
    if (s != NULL && (flags & MSG_PEEK) && s->family != AF_UNIX && s->state == SOCK_CONNECTED) {
        n = tcp_peek(s, buf, len, flags);
    } else {
        if (s != NULL && (flags & MSG_DONTWAIT) && !s->nonblock) {
            s->nonblock = 1;
            restore = 1;
        }
        n = read(sockfd, buf, len);
        if (restore) {
            s->nonblock = 0;
        }
    }
    if (n >= 0 && src_addr != NULL && addrlen != NULL) {
        s = sock_lookup(sockfd);
        if (s != NULL && s->peer_set) {
            ep_put(s, &s->peer, src_addr, addrlen);
        }
    }
    (void)flags;
    return n;
}

/* Cancellation points (pthread.c): accept and connect wait for netd. */
int __myos_accept(int sockfd, struct sockaddr *addr, socklen_t *addrlen) {
    __myos_cancel_enter();
    int r = accept_body(sockfd, addr, addrlen);
    __myos_cancel_leave();
    return r;
}

int __myos_connect(int sockfd, const struct sockaddr *addr, socklen_t addrlen) {
    __myos_cancel_enter();
    int r = connect_body(sockfd, addr, addrlen);
    __myos_cancel_leave();
    return r;
}

/* ---- UDP (SOCK_DGRAM over /net/udp; see the notes at the top) ---- */

/* errno for netd's word for what went wrong. */
static int netd_errno(const char *why) {
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

/* The library's fd of the datagrams with their headers: closed by exec,
 * where the socket's data fd goes on. */
static int udp_open_hdata(struct myos_sock *s) {
    char path[64];
    if (conv_path(path, sizeof path, s->proto_path, s->conv, "hdata") < 0
        || (s->hdata_fd = open(path, O_RDWR)) < 0) {
        errno = EIO;
        return -1;
    }
    (void)fcntl(s->hdata_fd, F_SETFD, FD_CLOEXEC);
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
    return netd_errno(b);
}

/* netd's answer to a udp_ctl command: "ok <addr>!<port>", the local
 * address after it, then the peer's if connected, or "fail <why>". */
static int udp_answer(struct myos_sock *s, const char *a) {
    if (strncmp(a, "ok ", 3) == 0) {
        struct ep peer;
        const char *rest = ep_parse(a + 3, &s->local);
        if (rest == NULL) {
            errno = EIO;
            return -1;
        }
        if (*rest == ' ' && ep_parse(rest + 1, &peer) != NULL) {
            s->peer = peer;
            s->peer_set = 1;
        }
        return 0;
    }
    errno = strncmp(a, "fail ", 5) == 0 ? netd_errno(a + 5) : EIO;
    return -1;
}

/* Run netd command `cmd` for `s` and wait for its answer (udp_answer):
 * tagged "#<n>" so the status that carries it is this command's. */
static int udp_ctl(struct myos_sock *s, const char *cmd) {
    char line[sizeof "bind " + EP_TEXT + sizeof " reuse v6only #4294967295"];
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
        char st[STATUS_LINE];
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

static int udp_bind(struct myos_sock *s, const struct ep *ep) {
    char cmd[sizeof "bind " + EP_TEXT + sizeof " reuse v6only"];
    int n = ep_cmd(cmd, sizeof cmd - sizeof " reuse v6only", "bind ", ep);
    if (n < 0) {
        errno = EINVAL;
        return -1;
    }
    cmd[n] = '\0';
    if (s->reuse) {
        strcat(cmd, " reuse");
    }
    if (ep->v6 && s->v6only) {
        strcat(cmd, " v6only");
    }
    return udp_ctl(s, cmd);
}

/* A port, as a first send binds it: an AF_INET6 socket's on ::. */
static int udp_autobind(struct myos_sock *s) {
    if (s->family != AF_INET6) {
        return udp_ctl(s, "autobind");
    }
    return udp_ctl(s, s->v6only ? "autobind :: v6only" : "autobind ::");
}

/* connect, or with AF_UNSPEC undo it: the peer is netd's. */
static int udp_connect(struct myos_sock *s, const struct sockaddr *addr, socklen_t addrlen) {
    struct ep ep;
    char cmd[sizeof "connect " + EP_TEXT];
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
        return 0;
    }
    if (ep_from(s, addr, addrlen, &ep) < 0) {
        return -1;
    }
    /* An AF_INET6 socket's port is on :: before its connect takes an
     * address. */
    if (s->family == AF_INET6 && s->local.port == 0 && udp_autobind(s) < 0) {
        return -1;
    }
    if (ep_cmd(cmd, sizeof cmd, "connect ", &ep) < 0) {
        errno = EINVAL;
        return -1;
    }
    if (udp_ctl(s, cmd) < 0) {
        return -1;
    }
    s->peer = ep;
    if (ep_unspecified(&s->peer)) {
        ep_localhost(&s->peer);
    }
    s->peer_set = 1;
    s->state = SOCK_CONNECTED;
    return 0;
}

/* send/sendto/write: `dest` NULL sends to the peer (a bare datagram on
 * data), any other destination is in the header (hdata). */
static ssize_t udp_send(struct myos_sock *s, const void *buf, size_t len, int flags,
    const struct sockaddr *dest, socklen_t destlen) {
    unsigned char msg[UDP_HDR + UDP_MAX];
    struct ep to;
    size_t off = 0;
    long r;
    (void)flags;
    /* No SIGPIPE: POSIX raises it for stream sockets only. */
    if (s->shut & SHUT_WR_BIT) {
        errno = EPIPE;
        return -1;
    }
    if (dest == NULL && !s->peer_set) {
        errno = EDESTADDRREQ;
        return -1;
    }
    if (dest != NULL && ep_from(s, dest, destlen, &to) < 0) {
        return -1;
    }
    if (len > UDP_MAX) {
        errno = EMSGSIZE;
        return -1;
    }
    /* The port a first send binds, for getsockname. */
    if (s->local.port == 0 && udp_autobind(s) < 0) {
        return -1;
    }
    if (dest != NULL) {
        memset(msg, 0, UDP_HDR);
        /* :: is all zeros, which with a port is this host to netd. */
        if (!(to.v6 && ep_unspecified(&to))) {
            ep_to_hdr(&to, msg);
        }
        memcpy(msg + 32, &to.port, 2);
        off = UDP_HDR;
    }
    memcpy(msg + off, buf, len);
    r = myos_syscall6(MYOS_SYS_PWRITE, off ? s->hdata_fd : s->data_fd, (long)(uintptr_t)msg,
        (long)(off + len), 0, 0, 0);
    if (r < 0) {
        int e = udp_take_error(s);
        errno = e ? e : ENOBUFS;
        return -1;
    }
    return (ssize_t)len;
}

/* recv/recvfrom/read: the next datagram (hdata: its sender in its header),
 * cut to `len` (MSG_TRUNC: its whole length returned). */
static ssize_t udp_recv(struct myos_sock *s, void *buf, size_t len, int flags,
    struct sockaddr *from, socklen_t *fromlen) {
    unsigned char msg[UDP_HDR + UDP_MAX];
    for (;;) {
        long r;
        if (s->shut & SHUT_RD_BIT) {
            return 0;
        }
        r = myos_syscall6(MYOS_SYS_PREAD, s->hdata_fd, (long)(uintptr_t)msg, (long)sizeof msg, 0, 0, 0);
        if (r == (long)MYOS_EINTR) {
            errno = EINTR;
            return -1;
        }
        /* A pending error leaves nothing to read (the kernel makes netfs's
         * failed read an empty one): it goes first, as on Linux. */
        if (r <= 0) {
            int e = udp_take_error(s);
            if (e != 0 || r < 0) {
                errno = e ? e : EIO;
                return -1;
            }
        }
        if (r >= UDP_HDR) {
            struct ep src;
            const unsigned char *data = msg + UDP_HDR;
            size_t n = (size_t)r - UDP_HDR;
            size_t copy;
            ep_from_hdr(msg, &src);
            memcpy(&src.port, msg + 32, 2);
            copy = n < len ? n : len;
            memcpy(buf, data, copy);
            ep_put(s, &src, from, fromlen);
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

/* sock_adopt for a /net/unix conversation, `rest` its "N/data": a stream
 * or a record socket as its ctl says, connected or a listener as its
 * status says. Its names are not known (getsockname: unnamed). */
static struct myos_sock *un_adopt(int fd, const char *rest) {
    char path[64];
    char buf[32];
    unsigned short id;
    struct myos_sock *s;
    ssize_t n;
    int ctl;
    if (parse_clone_id(rest, strcspn(rest, "/"), &id) < 0
        || strcmp(rest + strcspn(rest, "/"), "/data") != 0
        || (s = sock_alloc()) == NULL) {
        return NULL;
    }
    strcpy(s->proto_path, "/net/unix");
    s->conv = id;
    s->data_fd = fd;
    s->family = AF_UNIX;
    s->type = SOCK_STREAM;
    if (conv_path(path, sizeof path, s->proto_path, id, "ctl") == 0 && (ctl = open(path, O_RDONLY)) >= 0) {
        n = read(ctl, buf, sizeof buf);
        close(ctl);
        if (n > 0 && buf_has(buf, (size_t)n, "seqpacket")) {
            s->type = SOCK_SEQPACKET;
        }
    }
    s->state = SOCK_OPEN;
    if (listener_status(s, buf, sizeof buf) == 0) {
        if (strncmp(buf, "connected", 9) == 0 || strncmp(buf, "hangup", 6) == 0) {
            s->state = SOCK_CONNECTED;
            s->peer_set = 1;
        } else if (strncmp(buf, "announced", 9) == 0) {
            s->state = SOCK_LISTENING;
        }
    }
    return s;
}

/* A socket fd this process got by exec (or in an SCM_RIGHTS message),
 * which its table does not know: /proc/self/fd/N names /net/tcp/N/data,
 * /net/udp/N/data or /net/unix/N/data. Its state is the conversation's: a
 * TCP or unix status of "connected" or a listener's; a UDP socket's local
 * address and peer, asked of netd. What the process it came from kept to
 * itself (O_NONBLOCK, a TCP peer's address) is lost. NULL (errno
 * untouched) when the fd is no such socket. */
static struct myos_sock *sock_adopt(int fd) {
    char link[32] = "/proc/self/fd/";
    char path[64];
    char proto[16];
    unsigned short id;
    struct myos_sock *s;
    const char *rest;
    ssize_t n;
    int saved = errno;
    int type;
    if (fd < 0 || myos_u16_dec(link + 14, sizeof link - 14, (unsigned)fd) < 0) {
        return NULL;
    }
    n = readlink(link, path, sizeof path - 1);
    errno = saved;
    if (n <= 0) {
        return NULL;
    }
    path[n] = '\0';
    if (strncmp(path, "/net/unix/", 10) == 0) {
        s = un_adopt(fd, path + 10);
        errno = saved;
        return s;
    }
    if (strncmp(path, "/net/tcp/", 9) == 0) {
        type = SOCK_STREAM;
    } else if (strncmp(path, "/net/udp/", 9) == 0) {
        type = SOCK_DGRAM;
    } else {
        return NULL;
    }
    rest = path + 9;
    if (parse_clone_id(rest, strcspn(rest, "/"), &id) < 0
        || strcmp(rest + strcspn(rest, "/"), "/data") != 0
        || (type == SOCK_DGRAM && fd >= MYOS_MAX_SOCKS)
        || (s = sock_alloc()) == NULL) {
        return NULL;
    }
    memcpy(proto, path, 8);
    proto[8] = '\0';
    strcpy(s->proto_path, proto);
    s->conv = id;
    s->data_fd = fd;
    s->type = type;
    /* A TCP one's family is not known (taken for AF_INET); a UDP one's is
     * its address's. */
    s->family = AF_INET;
    s->state = SOCK_OPEN;
    if (type == SOCK_STREAM) {
        char st[STATUS_LINE];
        if (listener_status(s, st, sizeof st) == 0) {
            if (strncmp(st, "connected", 9) == 0) {
                s->state = SOCK_CONNECTED;
            } else if (strncmp(st, "listening", 9) == 0 || strncmp(st, "accepted", 8) == 0) {
                s->state = SOCK_LISTENING;
            }
        }
        errno = saved;
        return s;
    }
    /* UDP: its ctl and hdata, then what netd knows of it. */
    if (conv_path(path, sizeof path, s->proto_path, id, "ctl") < 0
        || (s->ctl_fd = open(path, O_RDWR)) < 0 || udp_open_hdata(s) < 0
        || udp_ctl(s, "local") < 0) {
        if (s->ctl_fd >= 0) {
            close(s->ctl_fd);
        }
        if (s->hdata_fd >= 0) {
            close(s->hdata_fd);
        }
        sock_free(s);
        errno = saved;
        return NULL;
    }
    (void)fcntl(s->ctl_fd, F_SETFD, FD_CLOEXEC);
    if (s->local.v6) {
        s->family = AF_INET6;
    }
    if (s->peer_set) {
        s->state = SOCK_CONNECTED;
    }
    dgram_fds[fd] = 1;
    errno = saved;
    return s;
}

MYOS_ALIAS(socket)
MYOS_ALIAS(bind)
MYOS_ALIAS(socketpair)
MYOS_ALIAS(listen)
MYOS_ALIAS(shutdown)
MYOS_ALIAS(setsockopt)
MYOS_ALIAS(getsockopt)
MYOS_ALIAS(getsockname)
MYOS_ALIAS(getpeername)
MYOS_ALIAS(send)
MYOS_ALIAS(recv)
MYOS_ALIAS(sendmsg)
MYOS_ALIAS(recvmsg)
MYOS_ALIAS(sendto)
MYOS_ALIAS(recvfrom)
MYOS_ALIAS(accept)
MYOS_ALIAS(connect)
