/* startx: run an X session on the console.
 *
 *   startx [program [arg...]] [-- server-arg...]
 *
 * Starts `Xfbdev :0` (with -br and the server args), waits until it accepts
 * connections, runs the program (dwm by default) with DISPLAY=:0, and stops
 * the server when the program exits, giving the console its screen and
 * keyboard back. The server takes the keyboard as it starts, so a session
 * has to be started like this, in one command: what is typed after
 * `Xfbdev &` goes to the server, not to the shell.
 *
 * Readiness is the X server's own handshake (as xinit uses it): started
 * with SIGUSR1 ignored, the server sends SIGUSR1 to its parent once its
 * sockets are listening.
 */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

#define MAX_ARGS 32

static volatile sig_atomic_t server_ready;
static volatile sig_atomic_t child_exited;

static void on_usr1(int sig) {
    (void)sig;
    server_ready = 1;
}

static void on_chld(int sig) {
    (void)sig;
    child_exited = 1;
}

int main(int argc, char **argv) {
    char *client[MAX_ARGS + 1] = {"dwm", NULL};
    char *server[MAX_ARGS + 4] = {"Xfbdev", ":0", "-br", NULL};
    int nc = 0, ns = 3, i = 1, status;
    struct sigaction sa;
    sigset_t block, old;

    for (; i < argc && strcmp(argv[i], "--") != 0; i++) {
        if (nc == MAX_ARGS) {
            fprintf(stderr, "startx: too many arguments\n");
            return 2;
        }
        client[nc++] = argv[i];
    }
    if (nc > 0) {
        client[nc] = NULL;
    }
    for (i++; i < argc; i++) {
        if (ns == MAX_ARGS + 3) {
            fprintf(stderr, "startx: too many server arguments\n");
            return 2;
        }
        server[ns++] = argv[i];
    }
    server[ns] = NULL;

    /* Handlers first, the signals blocked until sigsuspend: the server may
     * be ready (or gone) before the parent waits. */
    memset(&sa, 0, sizeof sa);
    sigemptyset(&sa.sa_mask);
    sa.sa_handler = on_usr1;
    sigaction(SIGUSR1, &sa, NULL);
    sa.sa_handler = on_chld;
    sigaction(SIGCHLD, &sa, NULL);
    sigemptyset(&block);
    sigaddset(&block, SIGUSR1);
    sigaddset(&block, SIGCHLD);
    sigprocmask(SIG_BLOCK, &block, &old);

    pid_t xpid = fork();
    if (xpid < 0) {
        perror("startx: fork");
        return 1;
    }
    if (xpid == 0) {
        sigprocmask(SIG_SETMASK, &old, NULL);
        signal(SIGUSR1, SIG_IGN);
        execvp(server[0], server);
        perror("startx: Xfbdev");
        _exit(127);
    }
    while (!server_ready && !child_exited) {
        sigsuspend(&old);
    }
    if (!server_ready) {
        waitpid(xpid, &status, 0);
        fprintf(stderr, "startx: the X server exited before it was ready\n");
        return 1;
    }

    pid_t cpid = fork();
    if (cpid < 0) {
        perror("startx: fork");
        kill(xpid, SIGTERM);
        waitpid(xpid, &status, 0);
        return 1;
    }
    if (cpid == 0) {
        sigprocmask(SIG_SETMASK, &old, NULL);
        setenv("DISPLAY", ":0", 1);
        execvp(client[0], client);
        fprintf(stderr, "startx: %s: %s\n", client[0], strerror(errno));
        _exit(127);
    }
    sigprocmask(SIG_SETMASK, &old, NULL);

    /* The session ends with the client (or with the server, if it dies). */
    int rc = 0;
    for (;;) {
        pid_t pid = waitpid(-1, &status, 0);
        if (pid == cpid) {
            rc = WIFEXITED(status) ? WEXITSTATUS(status) : 1;
            break;
        }
        if (pid == xpid) {
            fprintf(stderr, "startx: the X server exited\n");
            kill(cpid, SIGTERM);
            waitpid(cpid, &status, 0);
            return 1;
        }
        if (pid < 0 && errno != EINTR) {
            break;
        }
    }
    kill(xpid, SIGTERM);
    waitpid(xpid, &status, 0);
    return rc;
}
