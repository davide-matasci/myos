/* myos libgloss: minimal time(3) helpers (epoch from _gettimeofday). */

#include <time.h>
#include <sys/time.h>

time_t
time(time_t *t)
{
    struct timeval tv;

    if (gettimeofday(&tv, NULL) != 0) {
        if (t != NULL) {
            *t = 0;
        }
        return 0;
    }
    if (t != NULL) {
        *t = (time_t)tv.tv_sec;
    }
    return (time_t)tv.tv_sec;
}

struct tm *
localtime(const time_t *tp)
{
    static struct tm result;
    time_t sec = tp != NULL ? *tp : 0;

    if (gmtime_r(&sec, &result) == NULL) {
        return NULL;
    }
    return &result;
}
