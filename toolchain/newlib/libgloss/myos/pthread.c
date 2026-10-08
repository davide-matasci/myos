/*
 * POSIX threads (myos libgloss), on the kernel's threads (docs/threads.md):
 * thread_spawn starts one, wait_addr / wake_addr make its locks.
 *
 * A thread is one mapping, as the Rust std port lays it out: a guard page,
 * the stack, and on top its control block (struct __pthread), which the
 * thread pointer points at. The block holds the thread's own newlib state
 * (struct _reent: errno, stdio's), its key values and the word its joiner
 * waits on. Whoever is last frees the mapping: the joiner once the thread
 * has ended, or the thread itself when it was detached first. The main
 * thread's block is a static, and the thread pointer is only set once a
 * second thread starts: until then a program pays nothing.
 *
 * Every lock is one futex-style word (mutex_lock below): mutexes,
 * condition variables, once, and the locks newlib takes around malloc,
 * stdio, atexit, the environment and tz (__retarget_lock_*, build.sh).
 *
 * newlib supplies the declarations (<pthread.h>); build-libgloss.sh widens
 * pthread_t and pthread_mutex_t in the sysroot's <sys/_pthreadtypes.h>.
 * pthread_sigmask is in signal.c.
 */
#include <errno.h>
#include <limits.h>
#include <pthread.h>
#include <reent.h>
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/lock.h>
#include <sys/mman.h>
#include <sys/time.h>

#include "myos_syscalls.h"

#define SYS_THREAD_SPAWN 53
#define SYS_THREAD_EXIT 54
#define SYS_WAIT_ADDR 55
#define SYS_WAKE_ADDR 56
#define SYS_GETTID 57
#define SYS_SET_TP 89
#define SYS_YIELD 90

#define PAGE 4096UL
/* A thread's stack when the attributes name none; pages are only taken
 * when touched. */
#define DEFAULT_STACK (1UL << 20)
#define KEYS 64
#define DESTRUCTOR_ROUNDS 4
#ifndef PTHREAD_STACK_MIN
#define PTHREAD_STACK_MIN (16 * 1024)
#endif

/* struct __pthread.state: the thread runs and is joinable, */
#define RUNNING 0
/* has ended (its joiner frees the mapping), */
#define EXITED 1
/* or was detached (it frees the mapping itself). */
#define DETACHED 2

struct __pthread {
    struct __pthread *self; /* word 0: x86_64 reads the thread pointer at fs:0 */
    struct _reent *reent;
    uint32_t tid;
    uint32_t state;
    void *(*start)(void *);
    void *arg;
    void *result;
    struct _pthread_cleanup_context *cleanup;
    struct {
        const void *value;
        uint32_t generation; /* the key's when the value was set */
    } values[KEYS];
    char *base; /* the mapping; NULL for the main thread */
    size_t len;
    struct _reent own_reent;
};

static struct __pthread main_thread = { .self = &main_thread };

/* Set once a second thread exists: the thread pointer is then valid in
 * every thread. */
static int threaded;

/* Threads that have not ended: the last one to end exits the process. */
static uint32_t live = 1;

/* ---- the kernel's calls ---- */

/* Block while *addr holds `val`, at most `ns` nanoseconds (0: no limit).
 * Returns ETIMEDOUT once that has passed, else 0 (woken, the word already
 * differed, or a signal): callers check their condition again. */
static int futex_wait(uint32_t *addr, uint32_t val, unsigned long long ns) {
    return myos_syscall3(SYS_WAIT_ADDR, (long)addr, val, (long)ns) == 2 ? ETIMEDOUT : 0;
}

static void futex_wake(uint32_t *addr, int n) {
    myos_syscall2(SYS_WAKE_ADDR, (long)addr, n);
}

static struct __pthread *thread_pointer(void) {
    struct __pthread *t;
#if defined(__x86_64__)
    __asm__("mov %%fs:0, %0" : "=r"(t));
#elif defined(__aarch64__)
    __asm__("mrs %0, tpidr_el0" : "=r"(t));
#else
    __asm__("mv %0, tp" : "=r"(t));
#endif
    return t;
}

static struct __pthread *me(void) {
    if (threaded) {
        return thread_pointer();
    }
    if (!main_thread.tid) {
        main_thread.tid = (uint32_t)myos_syscall0(SYS_GETTID);
    }
    return &main_thread;
}

/* newlib's state for the calling thread: errno, stdio (build.sh). */
struct _reent *__getreent(void) {
    return threaded ? thread_pointer()->reent : _impure_ptr;
}

/* ---- the lock word ----
 * 0 free, 1 held, 2 held and maybe waited for (the holder wakes one
 * waiter when it lets go). */

static int lock_word_try(uint32_t *w) {
    uint32_t free = 0;
    return __atomic_compare_exchange_n(w, &free, 1, 0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED);
}

/* Take the word, waiting at most until `deadline` (a CLOCK_REALTIME
 * timespec, NULL: forever). */
static int lock_word(uint32_t *w, const struct timespec *deadline);

static void unlock_word(uint32_t *w) {
    if (__atomic_exchange_n(w, 0, __ATOMIC_RELEASE) == 2) {
        futex_wake(w, 1);
    }
}

/* Nanoseconds from now to `deadline`, 0 when it has passed. */
static unsigned long long ns_until(const struct timespec *deadline) {
    struct timeval now;
    gettimeofday(&now, 0);
    long long left = (long long)(deadline->tv_sec - now.tv_sec) * 1000000000LL
        + deadline->tv_nsec - (long long)now.tv_usec * 1000;
    return left > 0 ? (unsigned long long)left : 0;
}

static int lock_word(uint32_t *w, const struct timespec *deadline) {
    if (lock_word_try(w)) {
        return 0;
    }
    while (__atomic_exchange_n(w, 2, __ATOMIC_ACQUIRE) != 0) {
        unsigned long long ns = 0;
        if (deadline && !(ns = ns_until(deadline))) {
            return ETIMEDOUT;
        }
        futex_wait(w, 2, ns);
    }
    return 0;
}

/* ---- mutexes ---- */

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
    *m = (pthread_mutex_t)PTHREAD_MUTEX_INITIALIZER;
    if (attr && attr->is_initialized) {
        m->__type = (uint32_t)attr->type;
    }
    return 0;
}

int pthread_mutex_destroy(pthread_mutex_t *m) {
    return __atomic_load_n(&m->__state, __ATOMIC_RELAXED) ? EBUSY : 0;
}

/* Lock or try (deadline NULL and `try`), or wait to a deadline. A mutex
 * its holder locks again: a recursive one counts, a normal one waits for
 * itself (POSIX), any other is EDEADLK (EBUSY for a try). */
static int mutex_lock(pthread_mutex_t *m, int try, const struct timespec *deadline) {
    uint32_t tid = me()->tid;
    if (__atomic_load_n(&m->__owner, __ATOMIC_RELAXED) == tid
        && m->__type != PTHREAD_MUTEX_NORMAL) {
        if (m->__type != PTHREAD_MUTEX_RECURSIVE) {
            return try ? EBUSY : EDEADLK;
        }
        if (m->__count == UINT32_MAX) {
            return EAGAIN;
        }
        m->__count++;
        return 0;
    }
    int rc = try ? (lock_word_try(&m->__state) ? 0 : EBUSY) : lock_word(&m->__state, deadline);
    if (rc == 0) {
        __atomic_store_n(&m->__owner, tid, __ATOMIC_RELAXED);
    }
    return rc;
}

int pthread_mutex_lock(pthread_mutex_t *m) {
    return mutex_lock(m, 0, NULL);
}

int pthread_mutex_trylock(pthread_mutex_t *m) {
    return mutex_lock(m, 1, NULL);
}

int pthread_mutex_timedlock(pthread_mutex_t *m, const struct timespec *deadline) {
    if (deadline->tv_nsec < 0 || deadline->tv_nsec >= 1000000000L) {
        return EINVAL;
    }
    return mutex_lock(m, 0, deadline);
}

int pthread_mutex_unlock(pthread_mutex_t *m) {
    if (m->__type != PTHREAD_MUTEX_NORMAL
        && __atomic_load_n(&m->__owner, __ATOMIC_RELAXED) != me()->tid) {
        return EPERM;
    }
    if (m->__count) {
        m->__count--;
        return 0;
    }
    __atomic_store_n(&m->__owner, 0, __ATOMIC_RELAXED);
    unlock_word(&m->__state);
    return 0;
}

/* ---- condition variables ----
 * The word is a sequence number: a waiter sleeps while it is the one it
 * read before letting go of the mutex, and every signal moves it on. */

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
    __atomic_fetch_add(c, 1, __ATOMIC_RELEASE);
    futex_wake(c, 1);
    return 0;
}

int pthread_cond_broadcast(pthread_cond_t *c) {
    __atomic_fetch_add(c, 1, __ATOMIC_RELEASE);
    futex_wake(c, INT_MAX);
    return 0;
}

static int cond_wait(pthread_cond_t *c, pthread_mutex_t *m, const struct timespec *deadline) {
    uint32_t tid = me()->tid;
    if (m->__type != PTHREAD_MUTEX_NORMAL && m->__owner != tid) {
        return EPERM;
    }
    uint32_t seq = __atomic_load_n(c, __ATOMIC_RELAXED);
    /* Let go of the mutex, a recursive one's every lock. */
    uint32_t count = m->__count;
    m->__count = 0;
    m->__owner = 0;
    unlock_word(&m->__state);
    int rc = 0;
    unsigned long long ns = 0;
    if (deadline && !(ns = ns_until(deadline))) {
        rc = ETIMEDOUT;
    } else {
        rc = futex_wait(c, seq, ns);
    }
    lock_word(&m->__state, NULL);
    m->__owner = tid;
    m->__count = count;
    return rc;
}

int pthread_cond_wait(pthread_cond_t *c, pthread_mutex_t *m) {
    return cond_wait(c, m, NULL);
}

int pthread_cond_timedwait(pthread_cond_t *c, pthread_mutex_t *m, const struct timespec *deadline) {
    if (deadline->tv_nsec < 0 || deadline->tv_nsec >= 1000000000L) {
        return EINVAL;
    }
    return cond_wait(c, m, deadline);
}

/* ---- once ----
 * init_executed: 0 not run, 1 running (the others wait), 2 done. */

int pthread_once(pthread_once_t *once, void (*init)(void)) {
    uint32_t *w = (uint32_t *)&once->init_executed;
    uint32_t state = 0;
    if (__atomic_load_n(w, __ATOMIC_ACQUIRE) == 2) {
        return 0;
    }
    if (__atomic_compare_exchange_n(w, &state, 1, 0, __ATOMIC_ACQUIRE, __ATOMIC_ACQUIRE)) {
        init();
        __atomic_store_n(w, 2, __ATOMIC_RELEASE);
        futex_wake(w, INT_MAX);
        return 0;
    }
    while ((state = __atomic_load_n(w, __ATOMIC_ACQUIRE)) == 1) {
        futex_wait(w, 1, 0);
    }
    return 0;
}

/* ---- keys ----
 * The keys are global, their values per thread (struct __pthread). A key
 * made anew in a slot gets a new generation: a value set under an earlier
 * one reads as NULL. */

static struct {
    int used;
    uint32_t generation;
    void (*destructor)(void *);
} keys[KEYS];
/* Guards the key and pthread_atfork tables. */
static pthread_mutex_t tables_lock = PTHREAD_MUTEX_INITIALIZER;

int pthread_key_create(pthread_key_t *key, void (*destructor)(void *)) {
    int rc = EAGAIN;
    pthread_mutex_lock(&tables_lock);
    for (pthread_key_t k = 0; k < KEYS; k++) {
        if (!keys[k].used) {
            keys[k].used = 1;
            keys[k].generation++;
            keys[k].destructor = destructor;
            *key = k;
            rc = 0;
            break;
        }
    }
    pthread_mutex_unlock(&tables_lock);
    return rc;
}

int pthread_key_delete(pthread_key_t key) {
    int rc = EINVAL;
    pthread_mutex_lock(&tables_lock);
    if (key < KEYS && keys[key].used) {
        keys[key].used = 0;
        rc = 0;
    }
    pthread_mutex_unlock(&tables_lock);
    return rc;
}

void *pthread_getspecific(pthread_key_t key) {
    if (key >= KEYS || me()->values[key].generation != keys[key].generation) {
        return NULL;
    }
    return (void *)me()->values[key].value;
}

int pthread_setspecific(pthread_key_t key, const void *value) {
    if (key >= KEYS || !keys[key].used) {
        return EINVAL;
    }
    me()->values[key].value = value;
    me()->values[key].generation = keys[key].generation;
    return 0;
}

/* The ending thread's values through their keys' destructors, again while
 * destructors set new ones (POSIX: a few rounds). */
static void run_destructors(struct __pthread *t) {
    for (int round = 0; round < DESTRUCTOR_ROUNDS; round++) {
        int ran = 0;
        for (int k = 0; k < KEYS; k++) {
            void *value = pthread_getspecific((pthread_key_t)k);
            void (*destructor)(void *) = keys[k].used ? keys[k].destructor : NULL;
            if (value && destructor) {
                t->values[k].value = NULL;
                destructor(value);
                ran = 1;
            }
        }
        if (!ran) {
            break;
        }
    }
}

/* ---- threads ---- */

pthread_t pthread_self(void) {
    return me();
}

int pthread_equal(pthread_t a, pthread_t b) {
    return a == b;
}

/* Before a second thread starts, the main thread gets its thread pointer;
 * __getreent and me() read it from then on. */
static void start_threading(void) {
    if (threaded) {
        return;
    }
    struct __pthread *t = me();
    t->reent = _impure_ptr;
    myos_syscall1(SYS_SET_TP, (long)t);
    threaded = 1;
}

static void thread_main(struct __pthread *t) {
    t->tid = (uint32_t)myos_syscall0(SYS_GETTID);
    pthread_exit(t->start(t->arg));
}

int pthread_create(pthread_t *thread, const pthread_attr_t *attr, void *(*start)(void *), void *arg) {
    size_t stack = attr && attr->stacksize > 0 ? (size_t)attr->stacksize : DEFAULT_STACK;
    stack = (stack + PAGE - 1) & ~(PAGE - 1);
    size_t top = (sizeof(struct __pthread) + PAGE - 1) & ~(PAGE - 1);
    size_t len = PAGE + stack + top;
    char *base = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (base == MAP_FAILED) {
        return EAGAIN;
    }
    /* A stack overflow faults instead of running into another mapping. */
    mprotect(base, PAGE, PROT_NONE);
    /* The mapping is zeroed: no key values, no cleanup handlers. */
    struct __pthread *t = (struct __pthread *)(base + PAGE + stack);
    t->self = t;
    t->reent = &t->own_reent;
    _REENT_INIT_PTR(t->reent);
    t->start = start;
    t->arg = arg;
    t->base = base;
    t->len = len;
    t->state = attr && attr->detachstate == PTHREAD_CREATE_DETACHED ? DETACHED : RUNNING;

    start_threading();
    __atomic_fetch_add(&live, 1, __ATOMIC_RELAXED);
    /* thread_spawn's {entry, stack, arg, tls}: the stack grows down from
     * the control block, which is also the argument and thread pointer. */
    uint64_t spawn[4] = { (uintptr_t)thread_main, (uintptr_t)t, (uintptr_t)t, (uintptr_t)t };
    if (myos_syscall1(SYS_THREAD_SPAWN, (long)spawn) == (long)MYOS_SYSERR) {
        __atomic_fetch_sub(&live, 1, __ATOMIC_RELAXED);
        munmap(base, len);
        return EAGAIN;
    }
    *thread = t;
    return 0;
}

/* Mark `state` EXITED and end the thread: wake its joiner, or, when it was
 * detached, unmap `base..base + len`, `state` and the stack it runs on
 * included. Registers only, from the exchange on. */
static void __attribute__((noreturn)) thread_end(uint32_t *state, char *base, size_t len) {
#if defined(__x86_64__)
    register uint32_t *r12 __asm__("r12") = state;
    register char *r13 __asm__("r13") = base;
    register size_t r14 __asm__("r14") = len;
    __asm__ volatile(
        "mov $1, %%eax\n"          /* EXITED */
        "xchg %%eax, (%%r12)\n"
        "cmp $2, %%eax\n"          /* DETACHED */
        "je 2f\n"
        "mov %%r12, %%rdi\n"
        "mov $1, %%esi\n"
        "mov $56, %%eax\n"         /* wake_addr */
        "syscall\n"
        "jmp 3f\n"
        "2:\n"
        "mov %%r13, %%rdi\n"
        "mov %%r14, %%rsi\n"
        "mov $24, %%eax\n"         /* munmap */
        "syscall\n"
        "3:\n"
        "xor %%edi, %%edi\n"
        "mov $54, %%eax\n"         /* thread_exit */
        "syscall\n"
        "ud2\n"
        :
        : "r"(r12), "r"(r13), "r"(r14)
        : "memory");
#elif defined(__aarch64__)
    register uint32_t *x20 __asm__("x20") = state;
    register char *x21 __asm__("x21") = base;
    register size_t x22 __asm__("x22") = len;
    __asm__ volatile(
        "mov w9, #1\n"             /* EXITED */
        "1:\n"
        "ldaxr w10, [x20]\n"
        "stlxr w11, w9, [x20]\n"
        "cbnz w11, 1b\n"
        "cmp w10, #2\n"            /* DETACHED */
        "b.eq 2f\n"
        "mov x0, x20\n"
        "mov x1, #1\n"
        "mov x8, #56\n"            /* wake_addr */
        "svc #0\n"
        "b 3f\n"
        "2:\n"
        "mov x0, x21\n"
        "mov x1, x22\n"
        "mov x8, #24\n"            /* munmap */
        "svc #0\n"
        "3:\n"
        "mov x0, #0\n"
        "mov x8, #54\n"            /* thread_exit */
        "svc #0\n"
        "udf #0\n"
        :
        : "r"(x20), "r"(x21), "r"(x22)
        : "x0", "x1", "x8", "x9", "x10", "x11", "memory");
#else
    register uint32_t *s2 __asm__("s2") = state;
    register char *s3 __asm__("s3") = base;
    register size_t s4 __asm__("s4") = len;
    __asm__ volatile(
        "li t1, 1\n"               /* EXITED */
        "amoswap.w.aqrl t0, t1, (s2)\n"
        "li t1, 2\n"               /* DETACHED */
        "beq t0, t1, 2f\n"
        "mv a0, s2\n"
        "li a1, 1\n"
        "li a7, 56\n"              /* wake_addr */
        "ecall\n"
        "j 3f\n"
        "2:\n"
        "mv a0, s3\n"
        "mv a1, s4\n"
        "li a7, 24\n"              /* munmap */
        "ecall\n"
        "3:\n"
        "li a0, 0\n"
        "li a7, 54\n"              /* thread_exit */
        "ecall\n"
        "unimp\n"
        :
        : "r"(s2), "r"(s3), "r"(s4)
        : "t0", "t1", "a0", "a1", "a7", "memory");
#endif
    __builtin_unreachable();
}

void pthread_exit(void *result) {
    struct __pthread *t = me();
    t->result = result;
    while (t->cleanup) {
        struct _pthread_cleanup_context *c = t->cleanup;
        t->cleanup = c->_previous;
        c->_routine(c->_arg);
    }
    run_destructors(t);
    /* The last thread ends the process as returning from main does. */
    if (__atomic_sub_fetch(&live, 1, __ATOMIC_ACQ_REL) == 0) {
        exit(0);
    }
    if (t->base) {
        _reclaim_reent(t->reent);
    }
    /* From here a joiner may free the stack: no signal frame may land on
     * it. The main thread's block is static: the kernel keeps the process
     * until its other threads end (docs/threads.md). */
    sigset_t all;
    sigfillset(&all);
    pthread_sigmask(SIG_BLOCK, &all, NULL);
    thread_end(&t->state, t->base, t->len);
}

int pthread_join(pthread_t t, void **result) {
    if (t == me()) {
        return EDEADLK;
    }
    uint32_t state;
    while ((state = __atomic_load_n(&t->state, __ATOMIC_ACQUIRE)) == RUNNING) {
        futex_wait(&t->state, RUNNING, 0);
    }
    if (state != EXITED) {
        return EINVAL;
    }
    if (result) {
        *result = t->result;
    }
    /* It does not touch its mapping again. */
    if (t->base) {
        munmap(t->base, t->len);
    }
    return 0;
}

int pthread_detach(pthread_t t) {
    uint32_t state = __atomic_exchange_n(&t->state, DETACHED, __ATOMIC_ACQ_REL);
    if (state == DETACHED) {
        return EINVAL;
    }
    if (state == EXITED && t->base) {
        munmap(t->base, t->len);
    }
    return 0;
}

void pthread_yield(void) {
    myos_syscall0(SYS_YIELD);
}

/* ---- fork ---- */

#define ATFORK_MAX 16

static struct {
    void (*prepare)(void);
    void (*parent)(void);
    void (*child)(void);
} atfork[ATFORK_MAX];
static int atforks;

int pthread_atfork(void (*prepare)(void), void (*parent)(void), void (*child)(void)) {
    int rc = ENOMEM;
    pthread_mutex_lock(&tables_lock);
    if (atforks < ATFORK_MAX) {
        atfork[atforks].prepare = prepare;
        atfork[atforks].parent = parent;
        atfork[atforks].child = child;
        atforks++;
        rc = 0;
    }
    pthread_mutex_unlock(&tables_lock);
    return rc;
}

/* Around fork (stubs.c): the prepare handlers last registered first, then
 * the parent's or the child's in order. The child is one thread, the
 * forking one, under a tid of its own. */
void __myos_fork_prepare(void) {
    for (int i = atforks - 1; i >= 0; i--) {
        if (atfork[i].prepare) {
            atfork[i].prepare();
        }
    }
}

void __myos_fork_done(int child) {
    if (child) {
        me()->tid = (uint32_t)myos_syscall0(SYS_GETTID);
        live = 1;
    }
    for (int i = 0; i < atforks; i++) {
        void (*f)(void) = child ? atfork[i].child : atfork[i].parent;
        if (f) {
            f();
        }
    }
}

/* ---- attributes ---- */

int pthread_attr_init(pthread_attr_t *attr) {
    memset(attr, 0, sizeof *attr);
    attr->is_initialized = 1;
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
    if (size < PTHREAD_STACK_MIN || size > INT_MAX) {
        return EINVAL;
    }
    attr->stacksize = (int)size;
    return 0;
}

int pthread_attr_getstacksize(const pthread_attr_t *attr, size_t *size) {
    *size = attr->stacksize > 0 ? (size_t)attr->stacksize : DEFAULT_STACK;
    return 0;
}

/* ---- cancellation: not supported, a thread is never cancelled ---- */

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

/* Cleanup handlers: a stack per thread, run by pthread_exit. */
void _pthread_cleanup_push(struct _pthread_cleanup_context *ctx,
                           void (*routine)(void *), void *arg) {
    struct __pthread *t = me();
    ctx->_routine = routine;
    ctx->_arg = arg;
    ctx->_previous = t->cleanup;
    t->cleanup = ctx;
}

void _pthread_cleanup_pop(struct _pthread_cleanup_context *ctx, int execute) {
    me()->cleanup = ctx->_previous;
    if (execute) {
        ctx->_routine(ctx->_arg);
    }
}

/* ---- newlib's locks (build.sh: --enable-newlib-retargetable-locking) ----
 * A newlib lock is a mutex: normal or recursive. */

struct __lock {
    pthread_mutex_t m;
};

#define NEWLIB_LOCK(name, type) struct __lock __lock_##name = { { 0, type, 0, 0 } }
NEWLIB_LOCK(__sfp_recursive_mutex, PTHREAD_MUTEX_RECURSIVE);
NEWLIB_LOCK(__atexit_recursive_mutex, PTHREAD_MUTEX_RECURSIVE);
NEWLIB_LOCK(__at_quick_exit_mutex, PTHREAD_MUTEX_NORMAL);
NEWLIB_LOCK(__malloc_recursive_mutex, PTHREAD_MUTEX_RECURSIVE);
NEWLIB_LOCK(__env_recursive_mutex, PTHREAD_MUTEX_RECURSIVE);
NEWLIB_LOCK(__tz_mutex, PTHREAD_MUTEX_NORMAL);
NEWLIB_LOCK(__dd_hash_mutex, PTHREAD_MUTEX_NORMAL);
NEWLIB_LOCK(__arc4random_mutex, PTHREAD_MUTEX_NORMAL);

static void lock_new(_LOCK_T *lock, int type) {
    *lock = malloc(sizeof **lock);
    if (*lock) {
        (*lock)->m = (pthread_mutex_t)PTHREAD_MUTEX_INITIALIZER;
        (*lock)->m.__type = (uint32_t)type;
    }
}

void __retarget_lock_init(_LOCK_T *lock) {
    lock_new(lock, PTHREAD_MUTEX_NORMAL);
}

void __retarget_lock_init_recursive(_LOCK_T *lock) {
    lock_new(lock, PTHREAD_MUTEX_RECURSIVE);
}

void __retarget_lock_close(_LOCK_T lock) {
    free(lock);
}

void __retarget_lock_close_recursive(_LOCK_T lock) {
    free(lock);
}

void __retarget_lock_acquire(_LOCK_T lock) {
    if (lock) {
        pthread_mutex_lock(&lock->m);
    }
}

void __retarget_lock_acquire_recursive(_LOCK_T lock) {
    __retarget_lock_acquire(lock);
}

int __retarget_lock_try_acquire(_LOCK_T lock) {
    return lock ? pthread_mutex_trylock(&lock->m) == 0 : 1;
}

int __retarget_lock_try_acquire_recursive(_LOCK_T lock) {
    return __retarget_lock_try_acquire(lock);
}

void __retarget_lock_release(_LOCK_T lock) {
    if (lock) {
        pthread_mutex_unlock(&lock->m);
    }
}

void __retarget_lock_release_recursive(_LOCK_T lock) {
    __retarget_lock_release(lock);
}
