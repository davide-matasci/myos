/* startx: run an X session on the console.
 *
 *   startx [program [arg...]] [-- server-arg...]
 *
 * Starts `Xfbdev :0` (with -br, -dpi on a large screen, and the server
 * args), waits until it accepts
 * connections, runs the program (dwm by default) with DISPLAY=:0, and stops
 * the server when the program exits, giving the console its screen and
 * keyboard back. The server takes the keyboard as it starts, so a session
 * has to be started like this, in one command: what is typed after
 * `Xfbdev &` goes to the server, not to the shell.
 *
 * A large screen gets `-dpi` (unless the server args name one), so the
 * clients' fonts, which Xft sizes in points, keep their size on a laptop's
 * dense panel: kdrive takes 75 DPI whatever the screen, so dwm's bar was
 * as many pixels tall on a 1920x1080 panel as in QEMU's 1280x800 window.
 * The factor is the console's (modules/console/src/fb.rs, `scale_for`):
 * one more for every 450 lines, never below 80 columns of 8 pixels.
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

/* 75 times the console's scale for the screen /dev/fb/ctl describes; 0
 * when that is 1 (the server's own 75) or the screen is unknown. */
static int screen_dpi(void) {
    unsigned width = 0, height = 0;
    FILE *ctl = fopen("/dev/fb/ctl", "r");
    if (ctl == NULL) {
        return 0;
    }
    int n = fscanf(ctl, "%u %u", &width, &height);
    fclose(ctl);
    if (n != 2) {
        return 0;
    }
    unsigned scale = height / 450;
    if (scale > width / 640) {
        scale = width / 640;
    }
    return scale > 1 ? (int)(75 * scale) : 0;
}

int main(int argc, char **argv) {
    char *client[MAX_ARGS + 1] = {"dwm", NULL};
    char *server[MAX_ARGS + 6] = {"Xfbdev", ":0", "-br", NULL};
    char dpi[16];
    int has_dpi = 0;
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
        if (strcmp(argv[i], "-dpi") == 0) {
            has_dpi = 1;
        }
        server[ns++] = argv[i];
    }
    int d = has_dpi ? 0 : screen_dpi();
    if (d > 0) {
        snprintf(dpi, sizeof dpi, "%d", d);
        server[ns++] = "-dpi";
        server[ns++] = dpi;
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
