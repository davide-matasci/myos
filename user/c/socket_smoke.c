/* Prove outbound TCP via userspace BSD sockets (no socket syscall): an HTTP
 * GET of the launcher's test web server (src/test_www.rs, 10.0.2.2:8766
 * through QEMU's user network), whose answer is known, not a site on the
 * internet (github.com's outage failed CI #2527 here). */
#include <errno.h>
#include <arpa/inet.h>
#include <netdb.h>
#include <netinet/in.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static void die(const char *msg) {
    const char *prefix = "socket_smoke: ";
    write(STDOUT_FILENO, prefix, 14);
    write(STDOUT_FILENO, msg, strlen(msg));
    write(STDOUT_FILENO, "\n", 1);
    _exit(1);
}

/* Busy-wait backoff — no usleep in the freestanding guest. */
static void backoff(int attempt) {
    volatile unsigned n = 50000u * (unsigned)(attempt + 1) * (unsigned)(attempt + 1);
    while (n--) {
        /* spin */
    }
}

int main(void) {
    struct addrinfo hints, *res = NULL;
    int fd = -1;
    const char *req =
        "GET /hello.txt HTTP/1.1\r\n"
        "Host: 10.0.2.2\r\n"
        "Connection: close\r\n"
        "\r\n";
    char buf[512];
    /* The whole answer, for the check of its body (it is a few hundred bytes). */
    char resp[2048];
    size_t len = 0;
    ssize_t n;
    int got = 0;
    int i;
    int empty = 0;
    int attempt;

    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    if (getaddrinfo("10.0.2.2", "8766", &hints, &res) != 0 || res == NULL) {
        die("getaddrinfo fail");
    }

    /* CI bios sometimes loses the first SYN right after /ping — retry. */
    for (attempt = 0; attempt < 8; attempt++) {
        fd = socket(res->ai_family, res->ai_socktype, res->ai_protocol);
        if (fd < 0) {
            backoff(attempt);
            continue;
        }
        if (connect(fd, res->ai_addr, res->ai_addrlen) == 0) {
            break;
        }
        close(fd);
        fd = -1;
        backoff(attempt);
    }
    if (fd < 0) {
        freeaddrinfo(res);
        die("connect fail");
    }
    freeaddrinfo(res);

    if (send(fd, req, strlen(req), 0) < 0) {
        close(fd);
        die("send fail");
    }

    for (i = 0; i < 400000; i++) {
        n = recv(fd, buf, sizeof buf, 0);
        if (n < 0) {
            if (errno == EAGAIN || errno == EWOULDBLOCK) {
                if (got) {
                    empty++;
                    if (empty > 10000) {
                        break;
                    }
                }
                continue;
            }
            break;
        }
        if (n == 0) {
            break;
        }
        got = 1;
        empty = 0;
        write(STDOUT_FILENO, buf, (size_t)n);
        if (len + (size_t)n < sizeof resp) {
            memcpy(resp + len, buf, (size_t)n);
            len += (size_t)n;
        }
    }
    close(fd);

    if (!got) {
        die("no data");
    }
    resp[len] = '\0';
    if (strstr(resp, "hello from the myos test server") == NULL) {
        die("not the test server's hello");
    }
    write(STDOUT_FILENO, "\n[ OK ] socket\n", 15);
    return 0;
}
