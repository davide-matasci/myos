/* myos libgloss: the users are the security policy's (docs/security.md):
 * /proc/sys/security/users lists them, one "uid name home groups" line
 * each, and /proc/self/ctx says who the caller is ("uid user domain").
 * There is one group, root (gid 0). */

#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <pwd.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "myos_syscalls.h"

#define MYOS_USERS_FILE "/proc/sys/security/users"
#define MYOS_CTX_FILE "/proc/self/ctx"

/* The whole of a small /proc file into buf (NUL-terminated); its length. */
static int myos_read_text(const char *path, char *buf, size_t size) {
    int fd = open(path, O_RDONLY);
    size_t n = 0;
    ssize_t r;
    if (fd < 0) {
        return -1;
    }
    while (n + 1 < size && (r = read(fd, buf + n, size - 1 - n)) > 0) {
        n += (size_t)r;
    }
    close(fd);
    buf[n] = '\0';
    return (int)n;
}

static char pw_name_buf[64];
static char pw_dir_buf[256];
static struct passwd pwd_ent = {
    .pw_name = pw_name_buf,
    .pw_passwd = "",
    .pw_uid = 0,
    .pw_gid = 0,
    .pw_gecos = pw_name_buf,
    .pw_dir = pw_dir_buf,
    .pw_shell = "/bin/custom/sh",
};

/* The user on line `index` (index >= 0), or with uid `uid` (name NULL), or
 * named `name`: filled into pwd_ent. */
static struct passwd *myos_find_user(int index, uid_t uid, const char *name) {
    char users[4096];
    char *line;
    char *next;
    int i = 0;
    if (myos_read_text(MYOS_USERS_FILE, users, sizeof users) < 0) {
        return NULL;
    }
    for (line = users; *line != '\0'; line = next, i++) {
        char *f[3];
        int nf = 0;
        char *p = line;
        next = strchr(line, '\n');
        if (next != NULL) {
            *next++ = '\0';
        } else {
            next = line + strlen(line);
        }
        while (nf < 3 && *p != '\0') {
            while (*p == ' ') {
                *p++ = '\0';
            }
            if (*p == '\0') {
                break;
            }
            f[nf++] = p;
            while (*p != '\0' && *p != ' ') {
                p++;
            }
            if (*p == ' ') {
                *p++ = '\0';
            }
        }
        if (nf < 3) {
            continue;
        }
        if (index >= 0 ? i != index
                       : name != NULL ? strcmp(f[1], name) != 0 : (uid_t)strtoul(f[0], NULL, 10) != uid) {
            continue;
        }
        if (strlen(f[1]) >= sizeof pw_name_buf || strlen(f[2]) >= sizeof pw_dir_buf) {
            return NULL;
        }
        strcpy(pw_name_buf, f[1]);
        strcpy(pw_dir_buf, f[2]);
        pwd_ent.pw_uid = (uid_t)strtoul(f[0], NULL, 10);
        return &pwd_ent;
    }
    return NULL;
}

/* The caller's uid and user name, from /proc/self/ctx (0 and "root"
 * outside a policy). */
static char ctx_name[64];
static uid_t myos_ctx(void) {
    char ctx[128];
    char *sp;
    char *end;
    strcpy(ctx_name, "root");
    if (myos_read_text(MYOS_CTX_FILE, ctx, sizeof ctx) <= 0 || ctx[0] < '0' || ctx[0] > '9') {
        return 0;
    }
    sp = strchr(ctx, ' ');
    if (sp != NULL) {
        end = strchr(sp + 1, ' ');
        if (end != NULL && (size_t)(end - sp - 1) < sizeof ctx_name) {
            memcpy(ctx_name, sp + 1, (size_t)(end - sp - 1));
            ctx_name[end - sp - 1] = '\0';
        }
    }
    return (uid_t)strtoul(ctx, NULL, 10);
}

uid_t getuid(void) {
    return myos_ctx();
}

uid_t geteuid(void) {
    return myos_ctx();
}

char *getlogin(void) {
    (void)myos_ctx();
    return ctx_name;
}

int getlogin_r(char *buf, size_t bufsize) {
    size_t n;
    (void)myos_ctx();
    if (buf == NULL || bufsize == 0) {
        return EINVAL;
    }
    n = strlen(ctx_name) + 1;
    if (bufsize < n) {
        return ERANGE;
    }
    memcpy(buf, ctx_name, n);
    return 0;
}

int myos_setuser(const char *name, const char *password) {
    char buf[256];
    size_t nl = name != NULL ? strlen(name) : 0;
    size_t pl = password != NULL ? strlen(password) : 0;
    if (nl == 0 || nl + 1 + pl > sizeof buf) {
        errno = EINVAL;
        return -1;
    }
    memcpy(buf, name, nl);
    buf[nl] = '\0';
    if (pl != 0) {
        memcpy(buf + nl + 1, password, pl);
    }
    if (myos_syscall2(MYOS_SYS_SETUSER, (long)(uintptr_t)buf, (long)(nl + 1 + pl)) == (long)MYOS_SYSERR) {
        errno = EPERM;
        return -1;
    }
    return 0;
}

int myos_ns(const char *spec) {
    if (spec == NULL || myos_syscall2(MYOS_SYS_NS, (long)(uintptr_t)spec, (long)strlen(spec)) == (long)MYOS_SYSERR) {
        errno = EPERM;
        return -1;
    }
    return 0;
}

int myos_policy_load(const char *path) {
    if (path == NULL
        || myos_syscall2(MYOS_SYS_POLICY_LOAD, (long)(uintptr_t)path, (long)strlen(path)) == (long)MYOS_SYSERR) {
        errno = EPERM;
        return -1;
    }
    return 0;
}

static struct group grp_root = {
    .gr_name = "root",
    .gr_passwd = "*",
    .gr_gid = 0,
    .gr_mem = (char *[]){ "root", NULL },
};

struct passwd *
getpwuid(uid_t uid)
{
    return myos_find_user(-1, uid, NULL);
}

struct passwd *
getpwnam(const char *name)
{
    return name != NULL ? myos_find_user(-1, 0, name) : NULL;
}

struct group *
getgrgid(gid_t gid)
{
    if (gid == grp_root.gr_gid) {
        return &grp_root;
    }
    return NULL;
}

struct group *
getgrnam(const char *name)
{
    if (name != NULL && strcmp(name, grp_root.gr_name) == 0) {
        return &grp_root;
    }
    return NULL;
}

/* The policy's users in order: setpwent rewinds, getpwent walks them. */
static int pwd_pos = 0;

void
setpwent(void)
{
    pwd_pos = 0;
}

struct passwd *
getpwent(void)
{
    struct passwd *p = myos_find_user(pwd_pos, 0, NULL);
    if (p != NULL) {
        pwd_pos++;
    }
    return p;
}

void
endpwent(void)
{
    pwd_pos = 0;
}

/* One-entry group database (root), same contract as pwd for setgrent. */
static int grp_pos = 0;

void
setgrent(void)
{
    grp_pos = 0;
}

struct group *
getgrent(void)
{
    if (grp_pos == 0) {
        grp_pos = 1;
        return &grp_root;
    }
    return NULL;
}

void
endgrent(void)
{
    grp_pos = 0;
}

/* Pack a C string into *buf / *buflen; advance both. Returns 0 or ERANGE. */
static int
pack_str(char **dst, char **buf, size_t *buflen, const char *src)
{
    size_t n = strlen(src) + 1;
    if (*buflen < n) {
        return ERANGE;
    }
    memcpy(*buf, src, n);
    *dst = *buf;
    *buf += n;
    *buflen -= n;
    return 0;
}

static int
pack_passwd(struct passwd *dst, char *buf, size_t buflen, const struct passwd *src)
{
    char *p = buf;
    size_t left = buflen;
    if (pack_str(&dst->pw_name, &p, &left, src->pw_name) != 0
        || pack_str(&dst->pw_passwd, &p, &left, src->pw_passwd) != 0
        || pack_str(&dst->pw_gecos, &p, &left, src->pw_gecos) != 0
        || pack_str(&dst->pw_dir, &p, &left, src->pw_dir) != 0
        || pack_str(&dst->pw_shell, &p, &left, src->pw_shell) != 0) {
        return ERANGE;
    }
    dst->pw_uid = src->pw_uid;
    dst->pw_gid = src->pw_gid;
    return 0;
}

int
getpwuid_r(uid_t uid, struct passwd *pwd, char *buf, size_t buflen,
           struct passwd **result)
{
    if (pwd == NULL || buf == NULL || result == NULL) {
        return EINVAL;
    }
    const struct passwd *found = getpwuid(uid);
    if (found == NULL) {
        *result = NULL;
        return 0;
    }
    if (pack_passwd(pwd, buf, buflen, found) != 0) {
        *result = NULL;
        return ERANGE;
    }
    *result = pwd;
    return 0;
}

int
getpwnam_r(const char *name, struct passwd *pwd, char *buf, size_t buflen,
           struct passwd **result)
{
    if (pwd == NULL || buf == NULL || result == NULL) {
        return EINVAL;
    }
    const struct passwd *found = getpwnam(name);
    if (found == NULL) {
        *result = NULL;
        return 0;
    }
    if (pack_passwd(pwd, buf, buflen, found) != 0) {
        *result = NULL;
        return ERANGE;
    }
    *result = pwd;
    return 0;
}

static int
pack_group(struct group *dst, char *buf, size_t buflen, const struct group *src)
{
    char *p = buf;
    size_t left = buflen;
    char *mem_name;
    char **mem;
    size_t ptr_bytes;

    if (pack_str(&dst->gr_name, &p, &left, src->gr_name) != 0
        || pack_str(&dst->gr_passwd, &p, &left, src->gr_passwd) != 0
        || pack_str(&mem_name, &p, &left, "root") != 0) {
        return ERANGE;
    }
    /* Align pointer table for the architecture. */
    {
        uintptr_t al = (uintptr_t)p;
        uintptr_t pad = (sizeof(void *) - (al % sizeof(void *))) % sizeof(void *);
        if (left < pad) {
            return ERANGE;
        }
        p += pad;
        left -= pad;
    }
    ptr_bytes = 2 * sizeof(char *);
    if (left < ptr_bytes) {
        return ERANGE;
    }
    mem = (char **)(void *)p;
    mem[0] = mem_name;
    mem[1] = NULL;
    dst->gr_mem = mem;
    dst->gr_gid = src->gr_gid;
    return 0;
}

int
getgrgid_r(gid_t gid, struct group *grp, char *buf, size_t buflen,
           struct group **result)
{
    if (grp == NULL || buf == NULL || result == NULL) {
        return EINVAL;
    }
    if (gid != grp_root.gr_gid) {
        *result = NULL;
        return 0;
    }
    if (pack_group(grp, buf, buflen, &grp_root) != 0) {
        *result = NULL;
        return ERANGE;
    }
    *result = grp;
    return 0;
}

int
getgrnam_r(const char *name, struct group *grp, char *buf, size_t buflen,
           struct group **result)
{
    if (grp == NULL || buf == NULL || result == NULL) {
        return EINVAL;
    }
    if (name == NULL || strcmp(name, grp_root.gr_name) != 0) {
        *result = NULL;
        return 0;
    }
    if (pack_group(grp, buf, buflen, &grp_root) != 0) {
        *result = NULL;
        return ERANGE;
    }
    *result = grp;
    return 0;
}
