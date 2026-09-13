/* Prove outbound TCP via userspace BSD sockets (no socket syscall). */
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
        "GET / HTTP/1.1\r\n"
        "Host: example.com\r\n"
        "Connection: close\r\n"
        "\r\n";
    char buf[512];
    ssize_t n;
    int got = 0;
    int i;
    int empty = 0;
    int attempt;

    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    if (getaddrinfo("example.com", "80", &hints, &res) != 0 || res == NULL) {
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
    }
    close(fd);

    if (!got) {
        die("no data");
    }
    write(STDOUT_FILENO, "\n[ OK ] socket\n", 15);
    return 0;
}
