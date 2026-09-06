/*
 * Userspace BSD sockets over Plan 9 /net + netd (smoltcp).
 * No socket() syscall — outbound TCP (and UDP for DNS) via clone/ctl/data.
 */
#include <errno.h>
#include <fcntl.h>
#include "myos_fmt.h"
#include <string.h>
#include <unistd.h>

#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>

#include "myos_syscalls.h"

#define MYOS_MAX_SOCKS 16
#define STATUS_POLLS   400000

enum {
    SOCK_UNUSED = 0,
    SOCK_OPEN,
    SOCK_CONNECTED,
};

struct myos_sock {
    int used;
    int state;
    int type;       /* SOCK_STREAM / SOCK_DGRAM */
    int data_fd;    /* returned to the app; /net/.../data */
    int ctl_fd;     /* kept open through connect */
    unsigned short conv;
    char proto_path[16]; /* "/net/tcp" or "/net/udp" */
    struct sockaddr_in peer;
    int peer_set;
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

static int wait_connected(struct myos_sock *s) {
    char path[64];
    char sbuf[64];
    int i;
    if (conv_path(path, sizeof path, s->proto_path, s->conv, "status") < 0) {
        return -1;
    }
    for (i = 0; i < STATUS_POLLS; i++) {
        int st = open(path, O_RDONLY);
        if (st >= 0) {
            ssize_t nr = read(st, sbuf, sizeof sbuf);
            close(st);
            if (nr > 0 && buf_has(sbuf, (size_t)nr, "connected")) {
                return 0;
            }
            if (nr > 0 && (buf_has(sbuf, (size_t)nr, "error")
                    || buf_has(sbuf, (size_t)nr, "hangup"))) {
                errno = ECONNREFUSED;
                return -1;
            }
        }
    }
    errno = ETIMEDOUT;
    return -1;
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
    hangup_sock(s);
    /* data fd is being closed by _close caller */
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
    /* Outbound-only: accept INADDR_ANY / unspecified as no-op. */
    if (addr != NULL && addr->sa_family == AF_INET) {
        const struct sockaddr_in *in = (const struct sockaddr_in *)addr;
        if (in->sin_addr.s_addr != INADDR_ANY && in->sin_addr.s_addr != 0) {
            errno = EOPNOTSUPP;
            return -1;
        }
    }
    return 0;
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
    if (wait_connected(s) < 0) {
        return -1;
    }
    /* Match dns helper: close ctl after connected; hangup reopens ctl. */
    close(s->ctl_fd);
    s->ctl_fd = -1;
    s->peer = *in;
    s->peer_set = 1;
    s->state = SOCK_CONNECTED;
    return 0;
}

int listen(int sockfd, int backlog) {
    (void)backlog;
    if (sock_by_fd(sockfd) == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    errno = EOPNOTSUPP;
    return -1;
}

int accept(int sockfd, struct sockaddr *addr, socklen_t *addrlen) {
    (void)addr;
    (void)addrlen;
    if (sock_by_fd(sockfd) == NULL) {
        errno = ENOTSOCK;
        return -1;
    }
    errno = EOPNOTSUPP;
    return -1;
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
        *(int *)optval = 0;
        *optlen = sizeof(int);
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
    (void)flags;
    return read(sockfd, buf, len);
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
    (void)flags;
    n = read(sockfd, buf, len);
    if (n >= 0 && src_addr != NULL && addrlen != NULL) {
        struct myos_sock *s = sock_by_fd(sockfd);
        if (s != NULL && s->peer_set) {
            if (*addrlen > sizeof s->peer) {
                *addrlen = sizeof s->peer;
            }
            memcpy(src_addr, &s->peer, *addrlen);
        }
    }
    return n;
}
