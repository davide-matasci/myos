/* pthread-smoke: boot-CI guest test for the single-threaded pthread API
 * (libgloss pthread.c).
 *
 * Mutexes: the static initializer, EBUSY from trylock and EDEADLK from a
 * relock instead of a hang, EPERM unlocking an unlocked one, a recursive
 * one counting; once running once; keys; a timed condition wait sleeping
 * to its deadline with the mutex given back and retaken; pthread_create
 * failing with EAGAIN (the one thread is all there is). Prints [ OK ] pthread.
 */
#include <errno.h>
#include <pthread.h>
#include <stdio.h>
#include <sys/time.h>
#include <time.h>

static int fail(const char *what, int rc) {
    printf("[ FAIL ] pthread %s (rc %d)\n", what, rc);
    return 1;
}

static int once_runs;

static void once_fn(void) {
    once_runs++;
}

static void *never(void *arg) {
    return arg;
}

static long now_ms(void) {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return tv.tv_sec * 1000L + tv.tv_usec / 1000L;
}

int main(void) {
    int rc;
    static pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
    if ((rc = pthread_mutex_lock(&m)) != 0) {
        return fail("lock", rc);
    }
    if ((rc = pthread_mutex_trylock(&m)) != EBUSY) {
        return fail("trylock of a locked mutex", rc);
    }
    if ((rc = pthread_mutex_lock(&m)) != EDEADLK) {
        return fail("relock", rc);
    }
    if ((rc = pthread_mutex_destroy(&m)) != EBUSY) {
        return fail("destroy of a locked mutex", rc);
    }
    if ((rc = pthread_mutex_unlock(&m)) != 0) {
        return fail("unlock", rc);
    }
    if ((rc = pthread_mutex_unlock(&m)) != EPERM) {
        return fail("unlock of an unlocked mutex", rc);
    }

    pthread_mutexattr_t attr;
    pthread_mutex_t r;
    pthread_mutexattr_init(&attr);
    if ((rc = pthread_mutexattr_settype(&attr, PTHREAD_MUTEX_RECURSIVE)) != 0) {
        return fail("settype recursive", rc);
    }
    pthread_mutex_init(&r, &attr);
    pthread_mutexattr_destroy(&attr);
    for (int i = 0; i < 3; i++) {
        if ((rc = pthread_mutex_lock(&r)) != 0) {
            return fail("recursive lock", rc);
        }
    }
    if ((rc = pthread_mutex_trylock(&r)) != 0) {
        return fail("recursive trylock", rc);
    }
    for (int i = 0; i < 4; i++) {
        if ((rc = pthread_mutex_unlock(&r)) != 0) {
            return fail("recursive unlock", rc);
        }
    }
    if ((rc = pthread_mutex_unlock(&r)) != EPERM) {
        return fail("recursive unlock past zero", rc);
    }

    static pthread_once_t once = PTHREAD_ONCE_INIT;
    pthread_once(&once, once_fn);
    pthread_once(&once, once_fn);
    if (once_runs != 1) {
        return fail("once ran", once_runs);
    }

    pthread_key_t key;
    int value = 42;
    if ((rc = pthread_key_create(&key, NULL)) != 0) {
        return fail("key_create", rc);
    }
    if (pthread_getspecific(key) != NULL || pthread_setspecific(key, &value) != 0
        || pthread_getspecific(key) != &value) {
        return fail("key value", 0);
    }
    if ((rc = pthread_key_delete(key)) != 0) {
        return fail("key_delete", rc);
    }

    /* Nobody signals: the wait ends at its deadline, the mutex held again. */
    pthread_cond_t c = PTHREAD_COND_INITIALIZER;
    pthread_mutex_lock(&m);
    struct timeval tv;
    gettimeofday(&tv, NULL);
    long start = now_ms();
    struct timespec deadline = {tv.tv_sec, tv.tv_usec * 1000L + 200000000L};
    if (deadline.tv_nsec >= 1000000000L) {
        deadline.tv_sec++;
        deadline.tv_nsec -= 1000000000L;
    }
    if ((rc = pthread_cond_timedwait(&c, &m, &deadline)) != ETIMEDOUT) {
        return fail("timedwait", rc);
    }
    long waited = now_ms() - start;
    if (waited < 190) {
        printf("[ FAIL ] pthread timedwait returned after %ld ms of 200\n", waited);
        return 1;
    }
    if ((rc = pthread_mutex_trylock(&m)) != EBUSY) {
        return fail("mutex not retaken after timedwait", rc);
    }
    pthread_mutex_unlock(&m);

    pthread_t t;
    if ((rc = pthread_create(&t, NULL, never, NULL)) != EAGAIN) {
        return fail("pthread_create", rc);
    }
    if (!pthread_equal(pthread_self(), pthread_self())
        || (rc = pthread_join(pthread_self(), NULL)) != EDEADLK) {
        return fail("self", rc);
    }
    printf("[ OK ] pthread\n");
    return 0;
}
