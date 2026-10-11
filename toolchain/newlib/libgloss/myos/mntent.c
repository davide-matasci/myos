/* myos libgloss: setmntent, getmntent and friends over a mount table in
 * /proc/mounts' form: one mount per line, its source, mount point, type,
 * options and two numbers, separated by spaces or tabs, a space in a field
 * written \040 (and a tab \011, a backslash \134). */
#include <mntent.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

FILE *setmntent(const char *path, const char *mode) {
    return fopen(path, mode);
}

int endmntent(FILE *fp) {
    if (fp != NULL) {
        fclose(fp);
    }
    return 1;
}

/* Undo the octal escapes of `s` in place. */
static void unescape(char *s) {
    char *out = s;
    for (; *s != '\0'; s++) {
        if (s[0] == '\\' && s[1] >= '0' && s[1] <= '3' && s[2] >= '0' && s[2] <= '7' && s[3] >= '0'
            && s[3] <= '7') {
            *out++ = (char)((s[1] - '0') * 64 + (s[2] - '0') * 8 + (s[3] - '0'));
            s += 3;
        } else {
            *out++ = *s;
        }
    }
    *out = '\0';
}

/* The next field of `*p`, NUL-terminated in place; "" at the end. */
static char *field(char **p) {
    char *s = *p + strspn(*p, " \t\n");
    char *end = s + strcspn(s, " \t\n");
    if (*end != '\0') {
        *end++ = '\0';
    }
    *p = end;
    unescape(s);
    return s;
}

struct mntent *getmntent_r(FILE *fp, struct mntent *me, char *buf, int size) {
    while (fgets(buf, size, fp) != NULL) {
        char *p = buf;
        char *freq;
        char *passno;
        /* A line longer than the buffer is skipped whole. */
        if (strchr(buf, '\n') == NULL && !feof(fp)) {
            int c;
            while ((c = fgetc(fp)) != EOF && c != '\n') {
            }
            continue;
        }
        p += strspn(p, " \t");
        if (*p == '#' || *p == '\n' || *p == '\0') {
            continue;
        }
        me->mnt_fsname = field(&p);
        me->mnt_dir = field(&p);
        me->mnt_type = field(&p);
        me->mnt_opts = field(&p);
        freq = field(&p);
        passno = field(&p);
        if (*me->mnt_dir == '\0') {
            continue;
        }
        me->mnt_freq = atoi(freq);
        me->mnt_passno = atoi(passno);
        return me;
    }
    return NULL;
}

struct mntent *getmntent(FILE *fp) {
    static struct mntent me;
    static char buf[1024];
    return getmntent_r(fp, &me, buf, (int)sizeof(buf));
}

char *hasmntopt(const struct mntent *me, const char *opt) {
    size_t n = strlen(opt);
    char *s = me->mnt_opts;
    while (s != NULL && *s != '\0') {
        if (strncmp(s, opt, n) == 0 && (s[n] == '\0' || s[n] == ',' || s[n] == '=')) {
            return s;
        }
        s = strchr(s, ',');
        if (s != NULL) {
            s++;
        }
    }
    return NULL;
}
