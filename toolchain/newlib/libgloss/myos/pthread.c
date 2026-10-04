/*
 * POSIX threads for single-threaded programs (myos libgloss).
 *
 * C programs on myos have one thread: the kernel has threads (docs/threads.md)
 * but libc has no pthread_create yet, which here fails with EAGAIN. What the
 * rest of the API does is then exact for one thread: a mutex counts its
 * locks (relocking a non-recursive one is EDEADLK instead of a hang), a
 * condition variable has nobody to signal it (a timed wait sleeps to its
 * deadline, an untimed one is EDEADLK), once runs once, keys hold one value
 * each. Libraries that lock "in case" (libxcb, libX11) work unchanged.
 *
 * newlib supplies the types and declarations (<pthread.h>); pthread_sigmask
 * is in signal.c.
 */
#include <errno.h>
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/time.h>

int __myos_sleep_ns(unsigned long long ns, int flags); /* sleep.c */

/* The one thread's id. */
#define MAIN_THREAD ((pthread_t)1)

pthread_t pthread_self(void) {
    return MAIN_THREAD;
}

int pthread_equal(pthread_t a, pthread_t b) {
    return a == b;
}

int pthread_create(pthread_t *thread, const pthread_attr_t *attr,
                   void *(*start)(void *), void *arg) {
    (void)thread;
    (void)attr;
    (void)start;
    (void)arg;
    return EAGAIN;
}

int pthread_join(pthread_t thread, void **ret) {
    (void)ret;
    return thread == MAIN_THREAD ? EDEADLK : ESRCH;
}

int pthread_detach(pthread_t thread) {
    return thread == MAIN_THREAD ? 0 : ESRCH;
}

/* The last thread's exit ends the process, with status 0. */
void pthread_exit(void *ret) {
    (void)ret;
    exit(0);
}

/* Nothing to do at fork for one thread: no lock can be held by another. */
int pthread_atfork(void (*prepare)(void), void (*parent)(void), void (*child)(void)) {
    (void)prepare;
    (void)parent;
    (void)child;
    return 0;
}

int pthread_attr_init(pthread_attr_t *attr) {
    attr->is_initialized = 1;
    attr->stackaddr = 0;
    attr->stacksize = 0;
    attr->detachstate = PTHREAD_CREATE_JOINABLE;
    return 0;
}

int pthread_attr_destroy(pthread_attr_t *attr) {
    attr->is_initialized = 0;
    return 0;
}

int pthread_attr_setdetachstate(pthread_attr_t *attr, int state) {
    if (state != PTHREAD_CREATE_JOINABLE && state != PTHREAD_CREATE_DETACHED) {
        return EINVAL;
    }
    attr->detachstate = state;
    return 0;
}

int pthread_attr_getdetachstate(const pthread_attr_t *attr, int *state) {
    *state = attr->detachstate;
    return 0;
}

int pthread_attr_setstacksize(pthread_attr_t *attr, size_t size) {
    attr->stacksize = (int)size;
    return 0;
}

int pthread_attr_getstacksize(const pthread_attr_t *attr, size_t *size) {
    *size = (size_t)attr->stacksize;
    return 0;
}

/* Cancellation: the one thread is never cancelled. */
int pthread_setcancelstate(int state, int *old) {
    (void)state;
    if (old) {
        *old = PTHREAD_CANCEL_ENABLE;
    }
    return 0;
}

int pthread_setcanceltype(int type, int *old) {
    (void)type;
    if (old) {
        *old = PTHREAD_CANCEL_DEFERRED;
    }
    return 0;
}

void pthread_testcancel(void) {}

void _pthread_cleanup_push(struct _pthread_cleanup_context *ctx,
                           void (*routine)(void *), void *arg) {
    ctx->_routine = routine;
    ctx->_arg = arg;
}

void _pthread_cleanup_pop(struct _pthread_cleanup_context *ctx, int execute) {
    if (execute) {
        ctx->_routine(ctx->_arg);
    }
}

/* ---- mutexes ----
 * A pthread_mutex_t is 32 bits: the lock count in the low 16, the type
 * (PTHREAD_MUTEX_*) above. PTHREAD_MUTEX_INITIALIZER (all ones) reads as an
 * unlocked default mutex. */

#define COUNT_MASK 0xffffu
#define TYPE_SHIFT 16

static uint32_t mutex_word(const pthread_mutex_t *m) {
    return *m == _PTHREAD_MUTEX_INITIALIZER ? (uint32_t)PTHREAD_MUTEX_DEFAULT << TYPE_SHIFT : *m;
}

int pthread_mutexattr_init(pthread_mutexattr_t *attr) {
    attr->is_initialized = 1;
    attr->type = PTHREAD_MUTEX_DEFAULT;
    attr->recursive = 0;
    return 0;
}

int pthread_mutexattr_destroy(pthread_mutexattr_t *attr) {
    attr->is_initialized = 0;
    return 0;
}

int pthread_mutexattr_settype(pthread_mutexattr_t *attr, int type) {
    if (type < PTHREAD_MUTEX_NORMAL || type > PTHREAD_MUTEX_DEFAULT) {
        return EINVAL;
    }
    attr->type = type;
    attr->recursive = type == PTHREAD_MUTEX_RECURSIVE;
    return 0;
}

int pthread_mutexattr_gettype(const pthread_mutexattr_t *attr, int *type) {
    *type = attr->type;
    return 0;
}

int pthread_mutex_init(pthread_mutex_t *m, const pthread_mutexattr_t *attr) {
    int type = attr && attr->is_initialized ? attr->type : PTHREAD_MUTEX_DEFAULT;
    *m = (uint32_t)type << TYPE_SHIFT;
    return 0;
}

int pthread_mutex_destroy(pthread_mutex_t *m) {
    return mutex_word(m) & COUNT_MASK ? EBUSY : 0;
}

/* Relocking: recursive counts, any other type would wait for itself. */
static int mutex_relock(pthread_mutex_t *m, int busy) {
    uint32_t w = mutex_word(m);
    if (w >> TYPE_SHIFT != PTHREAD_MUTEX_RECURSIVE) {
        return busy;
    }
    if ((w & COUNT_MASK) == COUNT_MASK) {
        return EAGAIN;
    }
    *m = w + 1;
    return 0;
}

int pthread_mutex_lock(pthread_mutex_t *m) {
    uint32_t w = mutex_word(m);
    if (w & COUNT_MASK) {
        return mutex_relock(m, EDEADLK);
    }
    *m = w + 1;
    return 0;
}

int pthread_mutex_trylock(pthread_mutex_t *m) {
    uint32_t w = mutex_word(m);
    if (w & COUNT_MASK) {
        return mutex_relock(m, EBUSY);
    }
    *m = w + 1;
    return 0;
}

int pthread_mutex_unlock(pthread_mutex_t *m) {
    uint32_t w = mutex_word(m);
    if (!(w & COUNT_MASK)) {
        return EPERM;
    }
    *m = w - 1;
    return 0;
}

/* ---- condition variables: nobody can signal one ---- */

int pthread_condattr_init(pthread_condattr_t *attr) {
    attr->is_initialized = 1;
    attr->clock = CLOCK_REALTIME;
    return 0;
}

int pthread_condattr_destroy(pthread_condattr_t *attr) {
    attr->is_initialized = 0;
    return 0;
}

int pthread_cond_init(pthread_cond_t *c, const pthread_condattr_t *attr) {
    (void)attr;
    *c = 0;
    return 0;
}

int pthread_cond_destroy(pthread_cond_t *c) {
    (void)c;
    return 0;
}

int pthread_cond_signal(pthread_cond_t *c) {
    (void)c;
    return 0;
}

int pthread_cond_broadcast(pthread_cond_t *c) {
    (void)c;
    return 0;
}

/* Would wait forever: no other thread exists to signal. */
int pthread_cond_wait(pthread_cond_t *c, pthread_mutex_t *m) {
    (void)c;
    (void)m;
    return EDEADLK;
}

int pthread_cond_timedwait(pthread_cond_t *c, pthread_mutex_t *m, const struct timespec *abstime) {
    (void)c;
    if (abstime->tv_nsec < 0 || abstime->tv_nsec >= 1000000000L) {
        return EINVAL;
    }
    int rc = pthread_mutex_unlock(m);
    if (rc) {
        return rc;
    }
    for (;;) {
        struct timeval now;
        gettimeofday(&now, 0);
        long long left = (long long)(abstime->tv_sec - now.tv_sec) * 1000000000LL
            + abstime->tv_nsec - (long long)now.tv_usec * 1000;
        if (left <= 0) {
            break;
        }
        /* A signal ends the sleep early: go round and sleep the rest. */
        __myos_sleep_ns((unsigned long long)left, 0);
    }
    pthread_mutex_lock(m);
    return ETIMEDOUT;
}

/* ---- once and keys ---- */

int pthread_once(pthread_once_t *once, void (*init)(void)) {
    if (!once->init_executed) {
        once->init_executed = 1;
        init();
    }
    return 0;
}

#define MAX_KEYS 64

static struct {
    int used;
    const void *value;
} keys[MAX_KEYS];

int pthread_key_create(pthread_key_t *key, void (*destructor)(void *)) {
    /* Destructors run at a thread's exit, never for the main thread's. */
    (void)destructor;
    for (int i = 0; i < MAX_KEYS; i++) {
        if (!keys[i].used) {
            keys[i].used = 1;
            keys[i].value = 0;
            *key = (pthread_key_t)i;
            return 0;
        }
    }
    return EAGAIN;
}

int pthread_key_delete(pthread_key_t key) {
    if (key >= MAX_KEYS || !keys[key].used) {
        return EINVAL;
    }
    keys[key].used = 0;
    return 0;
}

void *pthread_getspecific(pthread_key_t key) {
    return key < MAX_KEYS && keys[key].used ? (void *)keys[key].value : 0;
}

int pthread_setspecific(pthread_key_t key, const void *value) {
    if (key >= MAX_KEYS || !keys[key].used) {
        return EINVAL;
    }
    keys[key].value = value;
    return 0;
}
