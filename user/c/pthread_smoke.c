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
 *     value for its joiner, a detached thread;
 *   - 150 threads started and joined, past the kernel's task slots, so
 *     each one's slot and stack are given back.
 * Prints [ OK ] pthread.
 */
#include <dirent.h>
#include <errno.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>

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
    pthread_attr_destroy(&attr);
    pthread_mutex_lock(&detached_lock);
    while (!detached_done) {
        pthread_cond_wait(&detached_ran, &detached_lock);
    }
    pthread_mutex_unlock(&detached_lock);

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

int main(void) {
    if (alone() || with_threads()) {
        return 1;
    }
    printf("[ OK ] pthread\n");
    return 0;
}
