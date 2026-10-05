/*
 * sec: the security policy from the shell (docs/security.md).
 *
 *   sec ctx                          the caller's uid, user and domain
 *   sec load FILE                    make FILE the policy (until reboot)
 *   sec as USER [-p PASSWORD] CMD... run CMD as USER, in its login domain
 *   sec ns BIND... -- CMD...         run CMD seeing only the BINDs, each
 *                                    PATH[=SOURCE][:RIGHTS] (SOURCE defaults
 *                                    to PATH, RIGHTS to all of the caller's:
 *                                    read,write,append,create,remove,exec,
 *                                    setattr,mount,signal)
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/myos_extra.h>
#include <unistd.h>

static void usage(void) {
    fprintf(stderr,
            "usage: sec ctx | sec load FILE | sec as USER [-p PASSWORD] CMD... | "
            "sec ns PATH[=SOURCE][:RIGHTS]... -- CMD...\n");
    exit(2);
}

static void run(char **argv) {
    execvp(argv[0], argv);
    fprintf(stderr, "sec: %s: %s\n", argv[0], strerror(errno));
    exit(127);
}

static int ctx(void) {
    char buf[128];
    int fd = open("/proc/self/ctx", O_RDONLY);
    ssize_t n;
    if (fd < 0 || (n = read(fd, buf, sizeof buf)) <= 0) {
        fprintf(stderr, "sec: /proc/self/ctx: %s\n", strerror(errno));
        return 1;
    }
    close(fd);
    fwrite(buf, 1, (size_t)n, stdout);
    return 0;
}

static int as(int argc, char **argv) {
    const char *user;
    const char *password = "";
    int i = 2;
    if (argc < 4) {
        usage();
    }
    user = argv[i++];
    if (strcmp(argv[i], "-p") == 0) {
        if (i + 2 >= argc) {
            usage();
        }
        password = argv[i + 1];
        i += 2;
    }
    if (myos_setuser(user, password) != 0) {
        fprintf(stderr, "sec: as %s: refused\n", user);
        return 1;
    }
    run(argv + i);
    return 127;
}

/* PATH[=SOURCE][:RIGHTS] as a kernel binding line "PATH SOURCE RIGHTS\n". */
static int add_binding(char *spec, size_t size, size_t *off, const char *arg) {
    char path[256];
    const char *rights = "all";
    const char *colon = strrchr(arg, ':');
    const char *eq;
    size_t plen = colon != NULL ? (size_t)(colon - arg) : strlen(arg);
    int n;
    if (colon != NULL) {
        rights = colon + 1;
    }
    if (plen == 0 || plen >= sizeof path) {
        return -1;
    }
    memcpy(path, arg, plen);
    path[plen] = '\0';
    eq = strchr(path, '=');
    if (eq != NULL) {
        path[eq - path] = '\0';
        n = snprintf(spec + *off, size - *off, "%s %s %s\n", path, eq + 1, rights);
    } else {
        n = snprintf(spec + *off, size - *off, "%s %s %s\n", path, path, rights);
    }
    if (n < 0 || (size_t)n >= size - *off) {
        return -1;
    }
    *off += (size_t)n;
    return 0;
}

static int ns(int argc, char **argv) {
    static char spec[4096];
    size_t off = 0;
    int i;
    for (i = 2; i < argc && strcmp(argv[i], "--") != 0; i++) {
        if (add_binding(spec, sizeof spec, &off, argv[i]) != 0) {
            fprintf(stderr, "sec: ns: %s: bad binding\n", argv[i]);
            return 2;
        }
    }
    if (i + 1 >= argc) {
        usage();
    }
    if (myos_ns(spec) != 0) {
        fprintf(stderr, "sec: ns: refused (a source the caller cannot name?)\n");
        return 1;
    }
    run(argv + i + 1);
    return 127;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        usage();
    }
    if (strcmp(argv[1], "ctx") == 0) {
        return ctx();
    }
    if (strcmp(argv[1], "load") == 0 && argc == 3) {
        if (myos_policy_load(argv[2]) != 0) {
            fprintf(stderr, "sec: load %s: refused (see the console)\n", argv[2]);
            return 1;
        }
        return 0;
    }
    if (strcmp(argv[1], "as") == 0) {
        return as(argc, argv);
    }
    if (strcmp(argv[1], "ns") == 0) {
        return ns(argc, argv);
    }
    usage();
    return 2;
}
