/* myos libgloss: flat single-user pwd/grp stubs (root only). */

#include <grp.h>
#include <pwd.h>
#include <stddef.h>
#include <string.h>

static struct passwd pwd_root = {
    .pw_name = "root",
    .pw_passwd = "",
    .pw_uid = 0,
    .pw_gid = 0,
    .pw_gecos = "root",
    .pw_dir = "/",
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
