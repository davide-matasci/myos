/* tty-smoke: the shell's terminal behaviour, driven through a pty.
 *
 * The boot tests run it after login (user/tests/run.sh). It starts an
 * interactive shell on the slave of a fresh pty (forkpty) and types at the
 * master what a person types at the console:
 *
 *  1. `echo histrecall_zz`, then Up, Left, `3`, Enter: the line editor must
 *     recall the previous command and insert into it (`histrecall_z3z`).
 *     With the editor dead the CSI bytes are swallowed or echoed as
 *     garbage and the edited command never runs.
 *  2. `x` then backspace then a command: erase must leave a clean argv.
 *  3. `cat | cat` then ^C: both children die (the right cat only because
 *     the kernel's pipe-read wait wakes on a fatal signal) and the shell
 *     survives (it ignores SIGINT) to run the next command.
 *
 * Reads block: the host's stall watchdog ends a hung run (poll does not
 * cover ptys). Prints `[ OK ] tty` and exits 0, or `[ FAIL ] tty <step>`.
 */
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

#include <pty.h>

static int master = -1;
static char acc[65536];
static size_t acc_len;
/* TTY_SMOKE_DEBUG=1: show every chunk read from the pty, escaped. */
static int debug;

static ssize_t read_master(void) {
    ssize_t n = read(master, acc + acc_len, sizeof acc - 1 - acc_len);
    if (debug && n > 0) {
        printf("[dbg] read %ld:", (long)n);
        for (ssize_t i = 0; i < n; i++) {
            unsigned char c = (unsigned char)acc[acc_len + i];
            if (c >= 0x20 && c < 0x7f) {
                putchar(c);
            } else {
                printf("\\x%02x", c);
            }
        }
        printf("\n");
        fflush(stdout);
    }
    return n;
}

/* Read from the master until `needle` shows up at the start of a line (the
 * shell's output, never the echo of the typed command, which follows the
 * prompt). Returns the position after it, or -1 on EOF/error. */
static int wait_line(const char *needle) {
    size_t from = 0;
    for (;;) {
        for (size_t i = from; i + strlen(needle) <= acc_len; i++) {
            if ((i == 0 || acc[i - 1] == '\n') && memcmp(acc + i, needle, strlen(needle)) == 0) {
                return (int)(i + strlen(needle));
            }
        }
        from = acc_len > strlen(needle) ? acc_len - strlen(needle) : 0;
        if (acc_len >= sizeof acc - 1) {
            return -1;
        }
        ssize_t n = read_master();
        if (n <= 0) {
            return -1;
        }
        acc_len += (size_t)n;
        acc[acc_len] = '\0';
    }
}

/* Read until the shell has printed its prompt again (`$ ` at the end). */
static int wait_prompt(void) {
    for (;;) {
        if (acc_len >= 2 && memcmp(acc + acc_len - 2, "$ ", 2) == 0) {
            return 0;
        }
        if (acc_len >= sizeof acc - 1) {
            return -1;
        }
        ssize_t n = read_master();
        if (n <= 0) {
            return -1;
        }
        acc_len += (size_t)n;
        acc[acc_len] = '\0';
    }
}

static void type(const char *s) {
    size_t n = strlen(s);
    while (n > 0) {
        ssize_t w = write(master, s, n);
        if (w <= 0) {
            break;
        }
        s += w;
        n -= (size_t)w;
    }
}

static void drop(void) {
    acc_len = 0;
    acc[0] = '\0';
}

static int fail(const char *step) {
    printf("[ FAIL ] tty %s\n", step);
    printf("--- pty output (%zu bytes) ---\n", acc_len);
    fwrite(acc, 1, acc_len, stdout);
    printf("\n---\n");
    return 1;
}

int main(void) {
    debug = getenv("TTY_SMOKE_DEBUG") != NULL;
    pid_t pid = forkpty(&master, NULL, NULL, NULL);
    if (pid < 0) {
        printf("[ FAIL ] tty forkpty (%s)\n", strerror(errno));
        return 1;
    }
    if (pid == 0) {
        setenv("TERM", "linux", 1);
        execl("/bin/sh", "sh", (char *)NULL);
        _exit(127);
    }
    if (wait_prompt() != 0) {
        return fail("first prompt");
    }
    /* 1. history recall and in-line editing. */
    drop();
    type("echo histrecall_zz\n");
    if (wait_line("histrecall_zz") < 0 || wait_prompt() != 0) {
        return fail("echo histrecall_zz");
    }
    drop();
    type("\x1b[A\x1b[D3\n");
    if (wait_line("histrecall_z3z") < 0 || wait_prompt() != 0) {
        return fail("arrow keys (Up, Left, insert)");
    }
    /* 2. backspace. */
    drop();
    type("x\x08/bin/sbase/echo bs-ok\n");
    if (wait_line("bs-ok") < 0 || wait_prompt() != 0 || strstr(acc, "not found") != NULL) {
        return fail("backspace");
    }
    /* 3. ^C on a foreground pipeline; the shell must still be there. */
    drop();
    type("cat | cat\n");
    usleep(800 * 1000);
    type("\x03");
    if (wait_prompt() != 0) {
        return fail("prompt after ^C");
    }
    drop();
    /* The quotes keep a surviving `cat | cat` from echoing the needle. */
    type("echo al''ive\n");
    if (wait_line("alive") < 0 || wait_prompt() != 0) {
        return fail("shell after ^C");
    }
    type("exit\n");
    int status = 0;
    waitpid(pid, &status, 0);
    close(master);
    printf("[ OK ] tty\n");
    return 0;
}
