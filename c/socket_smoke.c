/* Prove outbound TCP via userspace BSD sockets (no socket syscall). */
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

int main(void) {
    struct addrinfo hints, *res = NULL;
    int fd;
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

    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    if (getaddrinfo("example.com", "80", &hints, &res) != 0 || res == NULL) {
        die("getaddrinfo fail");
    }

    fd = socket(res->ai_family, res->ai_socktype, res->ai_protocol);
    if (fd < 0) {
        freeaddrinfo(res);
        die("socket fail");
    }
    if (connect(fd, res->ai_addr, res->ai_addrlen) < 0) {
        close(fd);
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
            break;
        }
        if (n == 0) {
            if (got) {
                empty++;
                if (empty > 10000) {
                    break;
                }
            }
            continue;
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
