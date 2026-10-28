/* multihome-smoke: boot-CI guest test for netd's virtual NICs (`/dev/tap`,
 * user/netd/src/nics.rs): a second link beside net0.
 *
 * The parent is the wire: it holds a free NIC's `prog`, brings its link up
 * and answers what netd sends on it as a small router 10.0.9.1 would (DHCP
 * offering 10.0.9.2/24, ARP, UDP echo on port 7, ping). The child checks
 * that netd took the NIC: `/net/ifaddrs` shows it with its lease,
 * `/net/ndb` keeps net0's (the wired link carries the default route), a
 * UDP socket connected to 10.0.9.1 has 10.0.9.2 as its source and gets its
 * datagram echoed, and a ping (`/net/icmp`) to 10.0.9.1 is answered.
 * When the child is done the parent lets go of `prog`, and the NIC
 * leaves `/net/ifaddrs`. Prints [ OK ] multihome.
 */
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

static const unsigned char PEER_MAC[6] = {0x02, 0x00, 0x00, 0x09, 0x00, 0x01};
static const unsigned char PEER_IP[4] = {10, 0, 9, 1};
static const unsigned char LEASE_IP[4] = {10, 0, 9, 2};

static int fail(const char *what) {
    printf("[ FAIL ] multihome %s (errno %d)\n", what, errno);
    fflush(stdout); /* the child leaves by _exit */
    return 1;
}

/* Whether `path` holds `needle`. */
static int file_has(const char *path, const char *needle) {
    char text[1024] = {0};
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return 0;
    ssize_t n = read(fd, text, sizeof text - 1);
    close(fd);
    return n > 0 && strstr(text, needle) != NULL;
}

/* Wait up to `ms` for `path` to hold `needle` (or not, `want` 0). */
static int wait_file(const char *path, const char *needle, int want, int ms) {
    for (int t = 0; t < ms; t += 100) {
        if (file_has(path, needle) == want)
            return 1;
        usleep(100000);
    }
    return 0;
}

static unsigned sum16(const unsigned char *p, size_t n, unsigned sum) {
    for (size_t i = 0; i + 1 < n; i += 2)
        sum += (p[i] << 8) | p[i + 1];
    if (n & 1)
        sum += p[n - 1] << 8;
    return sum;
}

static unsigned short fold(unsigned sum) {
    while (sum >> 16)
        sum = (sum & 0xffff) + (sum >> 16);
    return (unsigned short)~sum;
}

static void put16(unsigned char *p, unsigned v) {
    p[0] = v >> 8;
    p[1] = v;
}

/* An IPv4/UDP frame from the peer to `dst_mac`/`dst_ip` in `f`, the UDP
 * payload already at f + 42; its length. */
static size_t udp_frame(unsigned char *f, const unsigned char *dst_mac, const unsigned char *dst_ip,
                        unsigned sport, unsigned dport, size_t len) {
    memcpy(f, dst_mac, 6);
    memcpy(f + 6, PEER_MAC, 6);
    put16(f + 12, 0x0800);
    unsigned char *ip = f + 14, *udp = f + 34;
    memset(ip, 0, 20);
    ip[0] = 0x45;
    put16(ip + 2, 20 + 8 + len);
    ip[8] = 64;
    ip[9] = 17;
    memcpy(ip + 12, PEER_IP, 4);
    memcpy(ip + 16, dst_ip, 4);
    put16(ip + 10, fold(sum16(ip, 20, 0)));
    put16(udp, sport);
    put16(udp + 2, dport);
    put16(udp + 4, 8 + len);
    put16(udp + 6, 0);
    unsigned pseudo = sum16(ip + 12, 8, 17 + 8 + len);
    unsigned short c = fold(sum16(udp, 8 + len, pseudo));
    put16(udp + 6, c ? c : 0xffff);
    return 42 + len;
}

/* The DHCP reply (`type` 2 offer, 5 ack) to request `req` (its BOOTP part). */
static size_t dhcp_reply(unsigned char *f, const unsigned char *req, int type) {
    static const unsigned char bcast[6] = {0xff, 0xff, 0xff, 0xff, 0xff, 0xff};
    static const unsigned char bcast_ip[4] = {255, 255, 255, 255};
    unsigned char *b = f + 42;
    memset(b, 0, 240);
    b[0] = 2; /* BOOTREPLY */
    b[1] = 1;
    b[2] = 6;
    memcpy(b + 4, req + 4, 4);   /* xid */
    memcpy(b + 16, LEASE_IP, 4); /* yiaddr */
    memcpy(b + 28, req + 28, 16); /* chaddr */
    static const unsigned char magic[4] = {99, 130, 83, 99};
    memcpy(b + 236, magic, 4);
    unsigned char *o = b + 240;
    *o++ = 53, *o++ = 1, *o++ = type;
    *o++ = 54, *o++ = 4, memcpy(o, PEER_IP, 4), o += 4;
    *o++ = 51, *o++ = 4, *o++ = 0, *o++ = 0, *o++ = 0x0e, *o++ = 0x10;
    *o++ = 1, *o++ = 4, *o++ = 255, *o++ = 255, *o++ = 255, *o++ = 0;
    *o++ = 3, *o++ = 4, memcpy(o, PEER_IP, 4), o += 4;
    *o++ = 255;
    return udp_frame(f, bcast, bcast_ip, 67, 68, o - b);
}

/* The DHCP message type of a BOOTP request, 0 for none. */
static int dhcp_type(const unsigned char *b, size_t n) {
    for (size_t i = 240; i + 2 < n && b[i] != 255;) {
        if (b[i] == 0) {
            i++;
            continue;
        }
        if (b[i] == 53)
            return b[i + 2];
        i += 2 + b[i + 1];
    }
    return 0;
}

/* What the wire answered, for the report when the checks fail. */
static int n_arp, n_discover, n_request, n_echo, n_ping;

/* Print `path` under a heading. */
static void show(const char *path) {
    char text[1024] = {0};
    int fd = open(path, O_RDONLY);
    ssize_t got = fd < 0 ? -1 : read(fd, text, sizeof text - 1);
    if (fd >= 0)
        close(fd);
    printf("--- %s (%zd bytes)\n%s", path, got, got > 0 ? text : "");
    if (got > 0 && text[got - 1] != '\n')
        printf("\n");
}

/* Answer a frame netd sent, when it asks for an answer. */
static void answer(int prog, const unsigned char *f, size_t n) {
    unsigned char out[600];
    size_t len = 0;
    unsigned type = n >= 14 ? (f[12] << 8) | f[13] : 0;
    if (type == 0x0806 && n >= 42 && f[21] == 1 && !memcmp(f + 38, PEER_IP, 4)) {
        /* ARP who-has 10.0.9.1. */
        n_arp++;
        memcpy(out, f + 6, 6);
        memcpy(out + 6, PEER_MAC, 6);
        memcpy(out + 12, f + 12, 8);
        out[20] = 0;
        out[21] = 2;
        memcpy(out + 22, PEER_MAC, 6);
        memcpy(out + 28, PEER_IP, 4);
        memcpy(out + 32, f + 22, 10);
        len = 42;
    } else if (type == 0x0800 && n >= 42 && f[23] == 17) {
        const unsigned char *udp = f + 14 + (f[14] & 15) * 4;
        unsigned dport = (udp[2] << 8) | udp[3], ulen = (udp[4] << 8) | udp[5];
        const unsigned char *payload = udp + 8;
        if (ulen < 8 || payload + ulen - 8 > f + n)
            return;
        size_t plen = ulen - 8;
        if (dport == 67 && plen >= 240) {
            int t = dhcp_type(payload, plen);
            if (t == 1 || t == 3) /* discover, request */
                len = dhcp_reply(out, payload, t == 1 ? 2 : 5);
            if (t == 1)
                n_discover++;
            else if (t == 3)
                n_request++;
        } else if (dport == 7 && !memcmp(f + 30, PEER_IP, 4) && plen <= 500) {
            n_echo++;
            memcpy(out + 42, payload, plen);
            len = udp_frame(out, f + 6, f + 26, 7, (udp[0] << 8) | udp[1], plen);
        }
    } else if (type == 0x0800 && n >= 42 && n <= sizeof out && f[14] == 0x45 && f[23] == 1 && f[34] == 8 &&
               !memcmp(f + 30, PEER_IP, 4)) {
        /* An echo request to 10.0.9.1: the same packet back, as a reply. */
        n_ping++;
        memcpy(out, f + 6, 6);
        memcpy(out + 6, PEER_MAC, 6);
        memcpy(out + 12, f + 12, n - 12);
        memcpy(out + 26, PEER_IP, 4);
        memcpy(out + 30, f + 26, 4);
        out[24] = out[25] = 0;
        put16(out + 24, fold(sum16(out + 14, 20, 0)));
        size_t icmp_len = ((f[16] << 8) | f[17]) - 20; /* smoltcp: no IP options */
        if (icmp_len > n - 34)
            return;
        out[34] = 0;
        out[36] = out[37] = 0;
        put16(out + 36, fold(sum16(out + 34, icmp_len, 0)));
        len = n;
    }
    if (len)
        write(prog, out, len);
}

/* The checks, in the child, while the parent is the wire. */
static int check(const char *name) {
    char line[64];
    snprintf(line, sizeof line, "%s 10.0.9.2 255.255.255.0 up", name);
    if (!wait_file("/net/ifaddrs", line, 1, 15000))
        return fail("no lease on the virtual NIC");
    if (file_has("/net/ndb", "ip=10.0.9.2"))
        return fail("the virtual NIC took the default route");

    int s = socket(AF_INET, SOCK_DGRAM, 0);
    struct sockaddr_in peer = {.sin_family = AF_INET, .sin_port = htons(7)};
    memcpy(&peer.sin_addr, PEER_IP, 4);
    if (s < 0 || connect(s, (struct sockaddr *)&peer, sizeof peer) < 0)
        return fail("connect to 10.0.9.1");
    struct sockaddr_in me;
    socklen_t me_len = sizeof me;
    if (getsockname(s, (struct sockaddr *)&me, &me_len) < 0 || memcmp(&me.sin_addr, LEASE_IP, 4))
        return fail("source address is not the virtual NIC's");
    char got[32] = {0};
    for (int tries = 0; tries < 5; tries++) {
        if (send(s, "multihome", 9, 0) != 9)
            return fail("send");
        struct pollfd p = {.fd = s, .events = POLLIN};
        if (poll(&p, 1, 1000) == 1 && recv(s, got, sizeof got, 0) == 9)
            break;
    }
    if (memcmp(got, "multihome", 9))
        return fail("no echo from 10.0.9.1");
    close(s);
    /* An ICMP socket's source names its NIC too. */
    pid_t ping = fork();
    if (ping == 0) {
        execl("/bin/custom/ping", "ping", "10.0.9.1", (char *)NULL);
        _exit(127);
    }
    int st = 0;
    if (ping < 0 || waitpid(ping, &st, 0) != ping || !WIFEXITED(st) || WEXITSTATUS(st) != 0)
        return fail("no answer to a ping of 10.0.9.1");
    return 0;
}

int main(void) {
    char path[64], name[8];
    int n, prog = -1;
    for (n = 0; n < 4 && prog < 0; n++) {
        snprintf(path, sizeof path, "/dev/tap/%d/prog", n);
        prog = open(path, O_RDWR);
    }
    if (prog < 0)
        return fail("no free NIC");
    n--;
    snprintf(name, sizeof name, "tap%d", n);
    snprintf(path, sizeof path, "/dev/tap/%d/ctl", n);
    int ctl = open(path, O_RDWR);
    const char *mac = "mac 02:00:00:09:00:02";
    if (ctl < 0 || write(ctl, mac, strlen(mac)) < 0 || write(ctl, "up", 2) != 2)
        return fail("bring the link up");

    pid_t child = fork();
    if (child < 0)
        return fail("fork");
    if (child == 0)
        _exit(check(name));

    int status = 0;
    for (;;) {
        struct pollfd p = {.fd = prog, .events = POLLIN};
        if (poll(&p, 1, 100) == 1) {
            unsigned char f[2048];
            ssize_t got = read(prog, f, sizeof f);
            if (got > 0)
                answer(prog, f, got);
        }
        if (waitpid(child, &status, WNOHANG) == child)
            break;
    }
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        /* What the wire saw and what netd shows, for issue #649's "no
         * lease": which exchange stopped, and where. */
        printf("wire: %d arp, %d discover, %d request, %d echo, %d ping answered\n", n_arp, n_discover, n_request,
               n_echo, n_ping);
        show("/net/ifaddrs");
        show("/net/ndb");
        show(path);
        return 1;
    }

    close(ctl);
    close(prog);
    char line[16];
    snprintf(line, sizeof line, "%s ", name);
    if (!wait_file("/net/ifaddrs", line, 0, 5000))
        return fail("the NIC outlived its program in /net/ifaddrs");
    printf("[ OK ] multihome\n");
    return 0;
}
