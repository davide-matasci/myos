/* myos libgloss: flat single-user pwd/grp (root only). */

#include <errno.h>
#include <grp.h>
#include <pwd.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

static struct passwd pwd_root = {
    .pw_name = "root",
    .pw_passwd = "",
    .pw_uid = 0,
    .pw_gid = 0,
    .pw_gecos = "root",
    .pw_dir = "/root",
    .pw_shell = "/bin/custom/sh",
};

static struct group grp_root = {
    .gr_name = "root",
    .gr_passwd = "*",
    .gr_gid = 0,
    .gr_mem = (char *[]){ "root", NULL },
};

struct passwd *
getpwuid(uid_t uid)
{
    if (uid == pwd_root.pw_uid) {
        return &pwd_root;
    }
    return NULL;
}

struct passwd *
getpwnam(const char *name)
{
    if (name != NULL && strcmp(name, pwd_root.pw_name) == 0) {
        return &pwd_root;
    }
    return NULL;
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

/* One-entry user database (root): setpwent rewinds, getpwent enumerates it
 * exactly once per pass, endpwent resets. Matches the os-test setpwent
 * contract (getpwent + setpwent rewind must find the current uid twice). */
static int pwd_pos = 0;

void
setpwent(void)
{
    pwd_pos = 0;
}

struct passwd *
getpwent(void)
{
    if (pwd_pos == 0) {
        pwd_pos = 1;
        return &pwd_root;
    }
    return NULL;
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
    if (uid != pwd_root.pw_uid) {
        *result = NULL;
        return 0;
    }
    if (pack_passwd(pwd, buf, buflen, &pwd_root) != 0) {
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
    if (name == NULL || strcmp(name, pwd_root.pw_name) != 0) {
        *result = NULL;
        return 0;
    }
    if (pack_passwd(pwd, buf, buflen, &pwd_root) != 0) {
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
