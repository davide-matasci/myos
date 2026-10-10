/* pthread-smoke: boot-CI guest test for POSIX threads (libgloss pthread.c,
 * docs/threads.md).
 *
 * First on the main thread alone: mutexes (the static initializer, EBUSY
 * from trylock and EDEADLK from a relock instead of a hang, EPERM
 * unlocking an unlocked one, a recursive one counting), once, keys, a
 * timed condition wait that sleeps to its deadline with the mutex given
 * back and retaken. Then with threads:
 *   - a mutex under contention: four threads add to one counter;
 *   - a condition variable: a producer and two consumers pass items
 *     through a small queue, then a broadcast stops the consumers;
 *   - errno and key values per thread, a key's destructor at thread exit;
 *   - stdio and malloc from four threads at once (newlib's locks), a
 *     line written in three calls whole under flockfile; readdir_r;
 *   - once across threads, pthread_exit with a cleanup handler and a
 *     value for its joiner, a detached thread, detached threads that end
 *     at once;
 *   - 150 threads started and joined, past the kernel's task slots, so
 *     each one's slot and stack are given back;
 *   - a read-write lock shared by readers and writers, a barrier over
 *     rounds, a spin lock under contention, a condition wait on
 *     CLOCK_MONOTONIC;
 *   - cancellation: of a thread blocked in read() (also before it has
 *     run), of one in a condition wait (its cleanup handler finds the
 *     mutex locked), of one with it disabled (it sleeps on, then ends at
 *     pthread_testcancel), of an asynchronous one at its next syscall, of
 *     the caller itself;
 *   - fork while two threads keep malloc and stdio busy: each child can
 *     use both.
 * Prints [ OK ] pthread.
 */
#include <dirent.h>
#include <errno.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static int fail(const char *what, int rc) {
    printf("[ FAIL ] pthread %s (rc %d)\n", what, rc);
    return 1;
}

static long now_ms(void) {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return tv.tv_sec * 1000L + tv.tv_usec / 1000L;
}

static int once_runs;

static void once_fn(void) {
    once_runs++;
}

/* ---- one thread ---- */

static int alone(void) {
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
    /* A key made again in the same slot starts out NULL. */
    if ((rc = pthread_key_create(&key, NULL)) != 0 || pthread_getspecific(key) != NULL) {
        return fail("key made anew", rc);
    }
    pthread_key_delete(key);

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

    if (!pthread_equal(pthread_self(), pthread_self())
        || (rc = pthread_join(pthread_self(), NULL)) != EDEADLK) {
        return fail("self", rc);
    }
    return 0;
}

/* ---- threads ---- */

#define WORKERS 4
#define ADDS 20000

static pthread_mutex_t counter_lock = PTHREAD_MUTEX_INITIALIZER;
static long counter;

static void *add(void *arg) {
    (void)arg;
    for (int i = 0; i < ADDS; i++) {
        pthread_mutex_lock(&counter_lock);
        counter++;
        pthread_mutex_unlock(&counter_lock);
    }
    return NULL;
}

/* A queue of QUEUE slots between one producer and two consumers. */
#define QUEUE 4
#define ITEMS 2000

static pthread_mutex_t queue_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t queue_changed = PTHREAD_COND_INITIALIZER;
static int queue[QUEUE], queued, head, done;
static long consumed_sum;

static void *produce(void *arg) {
    (void)arg;
    for (int i = 1; i <= ITEMS; i++) {
        pthread_mutex_lock(&queue_lock);
        while (queued == QUEUE) {
            pthread_cond_wait(&queue_changed, &queue_lock);
        }
        queue[(head + queued++) % QUEUE] = i;
        pthread_cond_broadcast(&queue_changed);
        pthread_mutex_unlock(&queue_lock);
    }
    pthread_mutex_lock(&queue_lock);
    done = 1;
    pthread_cond_broadcast(&queue_changed);
    pthread_mutex_unlock(&queue_lock);
    return NULL;
}

static void *consume(void *arg) {
    (void)arg;
    pthread_mutex_lock(&queue_lock);
    for (;;) {
        while (queued == 0 && !done) {
            pthread_cond_wait(&queue_changed, &queue_lock);
        }
        if (queued == 0) {
            break;
        }
        consumed_sum += queue[head];
        head = (head + 1) % QUEUE;
        queued--;
        pthread_cond_broadcast(&queue_changed);
    }
    pthread_mutex_unlock(&queue_lock);
    return NULL;
}

/* errno, a key's value and its destructor, stdio and malloc, per thread. */
static pthread_key_t key;
static int destructed;
static FILE *shared_file;

static void destructor(void *value) {
    __atomic_fetch_add(&destructed, (int)(long)value, __ATOMIC_RELAXED);
}

static void *own_state(void *arg) {
    long n = (long)arg;
    pthread_setspecific(key, (void *)n);
    for (int i = 0; i < 200; i++) {
        char *p = malloc((size_t)(16 + i * n));
        if (!p) {
            return (void *)1;
        }
        memset(p, (int)n, (size_t)(16 + i * n));
        /* A line in three calls, whole under the stream's lock. */
        flockfile(shared_file);
        fprintf(shared_file, "thread %ld", n);
        fprintf(shared_file, " line ");
        fprintf(shared_file, "%d\n", i);
        funlockfile(shared_file);
        free(p);
    }
    /* errno stays this thread's while the others set theirs. */
    errno = (int)n;
    struct timespec pause = {0, 20 * 1000 * 1000};
    nanosleep(&pause, NULL);
    if (errno != n || pthread_getspecific(key) != (void *)n) {
        return (void *)2;
    }
    return NULL;
}

static void *once_racer(void *arg) {
    static pthread_once_t once = PTHREAD_ONCE_INIT;
    (void)arg;
    pthread_once(&once, once_fn);
    return NULL;
}

static int cleaned;

static void cleanup(void *arg) {
    cleaned = (int)(long)arg;
}

static void *exits(void *arg) {
    pthread_cleanup_push(cleanup, (void *)7);
    pthread_exit(arg);
    pthread_cleanup_pop(0);
    return NULL;
}

static pthread_mutex_t detached_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t detached_ran = PTHREAD_COND_INITIALIZER;
static int detached_done;

static void *detached(void *arg) {
    (void)arg;
    pthread_mutex_lock(&detached_lock);
    detached_done = 1;
    pthread_cond_signal(&detached_ran);
    pthread_mutex_unlock(&detached_lock);
    return NULL;
}

static void *identity(void *arg) {
    return arg;
}

static int quick_done;

static void *quick(void *arg) {
    (void)arg;
    __atomic_store_n(&quick_done, 1, __ATOMIC_RELEASE);
    return NULL;
}

static int with_threads(void) {
    pthread_t t[WORKERS];
    int rc;
    for (int i = 0; i < WORKERS; i++) {
        if ((rc = pthread_create(&t[i], NULL, add, NULL)) != 0) {
            return fail("create", rc);
        }
    }
    for (int i = 0; i < WORKERS; i++) {
        if ((rc = pthread_join(t[i], NULL)) != 0) {
            return fail("join", rc);
        }
    }
    if (counter != (long)WORKERS * ADDS) {
        printf("[ FAIL ] pthread counter %ld, want %ld\n", counter, (long)WORKERS * ADDS);
        return 1;
    }

    pthread_create(&t[0], NULL, produce, NULL);
    pthread_create(&t[1], NULL, consume, NULL);
    pthread_create(&t[2], NULL, consume, NULL);
    for (int i = 0; i < 3; i++) {
        pthread_join(t[i], NULL);
    }
    if (consumed_sum != (long)ITEMS * (ITEMS + 1) / 2) {
        printf("[ FAIL ] pthread queue passed a sum of %ld\n", consumed_sum);
        return 1;
    }

    shared_file = tmpfile();
    if (!shared_file || (rc = pthread_key_create(&key, destructor)) != 0) {
        return fail("tmpfile or key", rc);
    }
    for (int i = 0; i < WORKERS; i++) {
        pthread_create(&t[i], NULL, own_state, (void *)(long)(i + 1));
    }
    errno = 99;
    for (int i = 0; i < WORKERS; i++) {
        void *result;
        pthread_join(t[i], &result);
        if (result) {
            return fail("errno, key or malloc in a thread", (int)(long)result);
        }
    }
    if (errno != 99 || destructed != 1 + 2 + 3 + 4) {
        printf("[ FAIL ] pthread main errno %d, destructors summed %d\n", errno, destructed);
        return 1;
    }
    rewind(shared_file);
    int lines = 0;
    char line[64];
    while (fgets(line, sizeof line, shared_file)) {
        long tn;
        int ln;
        char end;
        lines += sscanf(line, "thread %ld line %d%c", &tn, &ln, &end) == 3 && end == '\n';
    }
    fclose(shared_file);
    if (lines != WORKERS * 200) {
        printf("[ FAIL ] pthread %d whole lines of %d from the threads\n", lines, WORKERS * 200);
        return 1;
    }

    /* readdir_r: the root's entries into the caller's buffer. */
    DIR *root = opendir("/");
    struct dirent entry, *e;
    int found = 0;
    while (root && readdir_r(root, &entry, &e) == 0 && e) {
        found |= e == &entry && strcmp(entry.d_name, "bin") == 0;
    }
    if (root) {
        closedir(root);
    }
    if (!found) {
        return fail("readdir_r found no /bin", 0);
    }

    once_runs = 0;
    for (int i = 0; i < WORKERS; i++) {
        pthread_create(&t[i], NULL, once_racer, NULL);
    }
    for (int i = 0; i < WORKERS; i++) {
        pthread_join(t[i], NULL);
    }
    if (once_runs != 1) {
        return fail("once across threads ran", once_runs);
    }

    void *result;
    pthread_create(&t[0], NULL, exits, (void *)5);
    if (pthread_join(t[0], &result) != 0 || result != (void *)5 || cleaned != 7) {
        return fail("pthread_exit", cleaned);
    }

    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
    if ((rc = pthread_create(&t[0], &attr, detached, NULL)) != 0) {
        return fail("create detached", rc);
    }
    pthread_mutex_lock(&detached_lock);
    while (!detached_done) {
        pthread_cond_wait(&detached_ran, &detached_lock);
    }
    pthread_mutex_unlock(&detached_lock);
    /* Detached threads that end at once: one may be gone, its block
     * unmapped, before pthread_create has stored its id there (issue
     * #390: the creator's store faulted). Each is waited for so the
     * kernel's task slots come back. */
    for (int i = 0; i < 50; i++) {
        __atomic_store_n(&quick_done, 0, __ATOMIC_RELAXED);
        if ((rc = pthread_create(&t[0], &attr, quick, NULL)) != 0) {
            return fail("create detached ending at once", rc);
        }
        while (!__atomic_load_n(&quick_done, __ATOMIC_ACQUIRE)) {
            pthread_yield();
        }
    }
    pthread_attr_destroy(&attr);

    for (long i = 0; i < 150; i++) {
        pthread_t one;
        if ((rc = pthread_create(&one, NULL, identity, (void *)i)) != 0) {
            printf("[ FAIL ] pthread thread %ld of 150 (rc %d)\n", i, rc);
            return 1;
        }
        if (pthread_join(one, &result) != 0 || result != (void *)i) {
            return fail("join of the many", (int)i);
        }
    }
    return 0;
}

/* ---- read-write locks, barriers, spin locks, clocks ---- */

static pthread_rwlock_t rw = PTHREAD_RWLOCK_INITIALIZER;
static long pair[2];
static int torn;

static void *rw_reader(void *arg) {
    (void)arg;
    for (int i = 0; i < 3000; i++) {
        pthread_rwlock_rdlock(&rw);
        if (pair[0] != pair[1]) {
            torn = 1;
        }
        pthread_rwlock_unlock(&rw);
    }
    return NULL;
}

static void *rw_writer(void *arg) {
    (void)arg;
    for (int i = 0; i < 1000; i++) {
        pthread_rwlock_wrlock(&rw);
        pair[0]++;
        pair[1]++;
        pthread_rwlock_unlock(&rw);
    }
    return NULL;
}

/* A read lock while main holds the write lock: it times out. */
static void *rw_timed(void *arg) {
    (void)arg;
    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    deadline.tv_nsec += 100000000L;
    if (deadline.tv_nsec >= 1000000000L) {
        deadline.tv_sec++;
        deadline.tv_nsec -= 1000000000L;
    }
    return (void *)(long)pthread_rwlock_timedrdlock(&rw, &deadline);
}

#define ROUNDS 50
static pthread_barrier_t barrier;
static int arrivals[ROUNDS], serials, barrier_bad;

static void *barrier_runner(void *arg) {
    (void)arg;
    for (int r = 0; r < ROUNDS; r++) {
        __atomic_fetch_add(&arrivals[r], 1, __ATOMIC_RELAXED);
        if (pthread_barrier_wait(&barrier) == PTHREAD_BARRIER_SERIAL_THREAD) {
            __atomic_fetch_add(&serials, 1, __ATOMIC_RELAXED);
        }
        /* Everyone arrived before anyone left. */
        if (__atomic_load_n(&arrivals[r], __ATOMIC_RELAXED) != WORKERS) {
            barrier_bad = 1;
        }
    }
    return NULL;
}

static pthread_spinlock_t spin;
static long spun;

static void *spinner(void *arg) {
    (void)arg;
    for (int i = 0; i < 20000; i++) {
        pthread_spin_lock(&spin);
        spun++;
        pthread_spin_unlock(&spin);
    }
    return NULL;
}

static int locks(void) {
    pthread_t t[WORKERS + 2];
    int rc;

    if ((rc = pthread_rwlock_rdlock(&rw)) != 0 || (rc = pthread_rwlock_rdlock(&rw)) != 0) {
        return fail("rdlock", rc);
    }
    if ((rc = pthread_rwlock_trywrlock(&rw)) != EBUSY) {
        return fail("trywrlock under readers", rc);
    }
    pthread_rwlock_unlock(&rw);
    pthread_rwlock_unlock(&rw);
    if ((rc = pthread_rwlock_wrlock(&rw)) != 0) {
        return fail("wrlock", rc);
    }
    if ((rc = pthread_rwlock_tryrdlock(&rw)) != EBUSY || (rc = pthread_rwlock_wrlock(&rw)) != EDEADLK) {
        return fail("relock of a write lock", rc);
    }
    void *result;
    pthread_create(&t[0], NULL, rw_timed, NULL);
    pthread_join(t[0], &result);
    if ((long)result != ETIMEDOUT) {
        return fail("timedrdlock under a writer", (int)(long)result);
    }
    if ((rc = pthread_rwlock_unlock(&rw)) != 0 || (rc = pthread_rwlock_unlock(&rw)) != EPERM) {
        return fail("rwlock unlock", rc);
    }
    for (int i = 0; i < WORKERS; i++) {
        pthread_create(&t[i], NULL, rw_reader, NULL);
    }
    pthread_create(&t[WORKERS], NULL, rw_writer, NULL);
    pthread_create(&t[WORKERS + 1], NULL, rw_writer, NULL);
    for (int i = 0; i < WORKERS + 2; i++) {
        pthread_join(t[i], NULL);
    }
    if (torn || pair[0] != 2000 || pair[1] != 2000 || pthread_rwlock_destroy(&rw) != 0) {
        printf("[ FAIL ] pthread rwlock: torn %d, pair %ld %ld\n", torn, pair[0], pair[1]);
        return 1;
    }

    if ((rc = pthread_barrier_init(&barrier, NULL, WORKERS)) != 0) {
        return fail("barrier_init", rc);
    }
    for (int i = 0; i < WORKERS; i++) {
        pthread_create(&t[i], NULL, barrier_runner, NULL);
    }
    for (int i = 0; i < WORKERS; i++) {
        pthread_join(t[i], NULL);
    }
    if (barrier_bad || serials != ROUNDS || pthread_barrier_destroy(&barrier) != 0) {
        printf("[ FAIL ] pthread barrier: early leave %d, %d serial threads of %d\n",
               barrier_bad, serials, ROUNDS);
        return 1;
    }

    pthread_spin_init(&spin, 0);
    for (int i = 0; i < WORKERS; i++) {
        pthread_create(&t[i], NULL, spinner, NULL);
    }
    for (int i = 0; i < WORKERS; i++) {
        pthread_join(t[i], NULL);
    }
    if (spun != (long)WORKERS * 20000 || pthread_spin_trylock(&spin) != 0
        || pthread_spin_trylock(&spin) != EBUSY) {
        printf("[ FAIL ] pthread spin lock counted %ld\n", spun);
        return 1;
    }

    /* A condition variable on CLOCK_MONOTONIC: its deadline is on that clock,
     * which counts from boot, far behind the time of day. */
    pthread_condattr_t ca;
    clockid_t clock;
    pthread_condattr_init(&ca);
    if (pthread_condattr_setclock(&ca, (clockid_t)42) != EINVAL
        || (rc = pthread_condattr_setclock(&ca, CLOCK_MONOTONIC)) != 0
        || pthread_condattr_getclock(&ca, &clock) != 0 || clock != CLOCK_MONOTONIC) {
        return fail("condattr clock", rc);
    }
    pthread_cond_t c;
    pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
    pthread_cond_init(&c, &ca);
    struct timespec mono, wall;
    clock_gettime(CLOCK_MONOTONIC, &mono);
    clock_gettime(CLOCK_REALTIME, &wall);
    if (mono.tv_sec >= wall.tv_sec) {
        return fail("CLOCK_MONOTONIC is the time of day", (int)mono.tv_sec);
    }
    mono.tv_nsec += 200000000L;
    if (mono.tv_nsec >= 1000000000L) {
        mono.tv_sec++;
        mono.tv_nsec -= 1000000000L;
    }
    long start = now_ms();
    pthread_mutex_lock(&m);
    rc = pthread_cond_timedwait(&c, &m, &mono);
    pthread_mutex_unlock(&m);
    long waited = now_ms() - start;
    if (rc != ETIMEDOUT || waited < 190 || waited > 5000) {
        printf("[ FAIL ] pthread monotonic timedwait: rc %d after %ld ms of 200\n", rc, waited);
        return 1;
    }
    return 0;
}

/* ---- cancellation ---- */

static int pipe_fds[2];
static int cancel_cleaned;

static void count_cleanup(void *arg) {
    (void)arg;
    cancel_cleaned++;
}

static void *blocked_read(void *arg) {
    (void)arg;
    char c;
    pthread_cleanup_push(count_cleanup, NULL);
    read(pipe_fds[0], &c, 1); /* nobody writes */
    pthread_cleanup_pop(0);
    return NULL;
}

static pthread_mutex_t cw_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cw_cond = PTHREAD_COND_INITIALIZER;
static int cw_locked_in_cleanup;

static void cw_cleanup(void *arg) {
    (void)arg;
    /* The mutex is the cancelled thread's again: unlocking it succeeds. */
    cw_locked_in_cleanup = pthread_mutex_unlock(&cw_lock) == 0;
}

static void *cond_waiter(void *arg) {
    (void)arg;
    pthread_mutex_lock(&cw_lock);
    pthread_cleanup_push(cw_cleanup, NULL);
    for (;;) {
        pthread_cond_wait(&cw_cond, &cw_lock); /* nobody signals */
    }
    pthread_cleanup_pop(0);
    return NULL;
}

static long slept_ms;

static void *disabled_sleeper(void *arg) {
    (void)arg;
    pthread_setcancelstate(PTHREAD_CANCEL_DISABLE, NULL);
    long start = now_ms();
    struct timespec t = {0, 300 * 1000 * 1000};
    nanosleep(&t, NULL); /* not cut short: the cancellation waits */
    slept_ms = now_ms() - start;
    pthread_setcancelstate(PTHREAD_CANCEL_ENABLE, NULL);
    pthread_testcancel();
    return NULL;
}

static void *async_spinner(void *arg) {
    (void)arg;
    pthread_setcanceltype(PTHREAD_CANCEL_ASYNCHRONOUS, NULL);
    for (;;) {
        getpid(); /* signals act on the way out of a syscall */
    }
    return NULL;
}

static void *self_cancel(void *arg) {
    (void)arg;
    pthread_cancel(pthread_self());
    pthread_testcancel();
    return NULL;
}

static void sleep_ms(long ms) {
    struct timespec t = {0, ms * 1000 * 1000};
    nanosleep(&t, NULL);
}

static int cancelled(void *(*fn)(void *), int wait_ms, const char *what) {
    pthread_t t;
    void *result = NULL;
    int rc;
    if ((rc = pthread_create(&t, NULL, fn, NULL)) != 0) {
        return fail(what, rc);
    }
    if (wait_ms) {
        sleep_ms(wait_ms);
        if ((rc = pthread_cancel(t)) != 0) {
            return fail(what, rc);
        }
    }
    if ((rc = pthread_join(t, &result)) != 0 || result != PTHREAD_CANCELED) {
        printf("[ FAIL ] pthread cancel %s: join %d, result %p\n", what, rc, result);
        return 1;
    }
    return 0;
}

static int cancellation(void) {
    if (pipe(pipe_fds) != 0) {
        return fail("pipe", errno);
    }
    if (cancelled(blocked_read, 50, "in read") || cancel_cleaned != 1) {
        return cancel_cleaned != 1 ? fail("read's cleanup handler ran", cancel_cleaned) : 1;
    }
    /* Cancelled before it has run at all: the signal goes to the thread
     * (whose tid pthread_create knows), not to the process group. */
    pthread_t early;
    void *result;
    if (pthread_create(&early, NULL, blocked_read, NULL) != 0 || pthread_cancel(early) != 0
        || pthread_join(early, &result) != 0 || result != PTHREAD_CANCELED) {
        return fail("cancel right after create", 0);
    }
    close(pipe_fds[0]);
    close(pipe_fds[1]);
    if (cancelled(cond_waiter, 50, "in a condition wait") || !cw_locked_in_cleanup
        || pthread_mutex_trylock(&cw_lock) != 0) {
        return fail("condition wait's mutex in its cleanup handler", cw_locked_in_cleanup);
    }
    pthread_mutex_unlock(&cw_lock);
    if (cancelled(disabled_sleeper, 50, "while disabled") || slept_ms < 290) {
        printf("[ FAIL ] pthread disabled cancellation cut a sleep to %ld ms of 300\n", slept_ms);
        return 1;
    }
    if (cancelled(async_spinner, 50, "asynchronous") || cancelled(self_cancel, 0, "of itself")) {
        return 1;
    }
    int old;
    if (pthread_setcancelstate(7, &old) != EINVAL || pthread_setcanceltype(7, &old) != EINVAL) {
        return fail("cancel state or type of 7", 0);
    }
    return 0;
}

/* ---- fork while other threads use malloc and stdio ---- */

static int busy_stop;

static void *busy(void *arg) {
    FILE *f = arg;
    while (!__atomic_load_n(&busy_stop, __ATOMIC_RELAXED)) {
        char *p = malloc(100);
        fprintf(f, "x");
        free(p);
    }
    return NULL;
}

static int fork_busy(void) {
    FILE *f = tmpfile();
    pthread_t t[2];
    if (!f) {
        return fail("tmpfile for fork", errno);
    }
    for (int i = 0; i < 2; i++) {
        pthread_create(&t[i], NULL, busy, f);
    }
    int bad = 0;
    for (int i = 0; i < 20 && !bad; i++) {
        pid_t pid = fork();
        if (pid == 0) {
            /* Would wait forever on a lock a thread of the parent held. */
            char *p = malloc(4096);
            fprintf(f, "child");
            fflush(f);
            _exit(p ? 0 : 1);
        }
        int status = 0;
        bad = pid < 0 || waitpid(pid, &status, 0) != pid || !WIFEXITED(status)
            || WEXITSTATUS(status) != 0;
    }
    __atomic_store_n(&busy_stop, 1, __ATOMIC_RELAXED);
    for (int i = 0; i < 2; i++) {
        pthread_join(t[i], NULL);
    }
    fclose(f);
    return bad ? fail("fork with busy threads", bad) : 0;
}

int main(void) {
    if (alone() || with_threads() || locks() || cancellation() || fork_busy()) {
        return 1;
    }
    printf("[ OK ] pthread\n");
    return 0;
}
