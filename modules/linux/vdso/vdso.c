/*
 * The Linux layer's vDSO (docs/linux-compat.md, "vDSO"): clock_gettime of
 * the monotonic clocks read from the counter the kernel's monotonic clock
 * runs on, without a syscall. JavaScriptCore's collector, for one, reads
 * the clock every few objects it marks, millions of times a collection;
 * as a syscall each read cost more than the marking.
 *
 * The time is the kernel's own, to the nanosecond (`KernelApi::user_clock`
 * and `time::counter_to_ns`): a program mixing this with the syscall never
 * sees it go back. Any other clock answers -ENOSYS, and musl makes the
 * syscall then.
 */

typedef unsigned long u64;

struct timespec {
	long tv_sec;
	long tv_nsec;
};

#define ENOSYS 38
#define NSEC_PER_SEC 1000000000UL

/*
 * The counter's value at nanosecond 0 and its rate, written by the module
 * into its copy of this image before mapping it (`vdso.rs` finds the
 * block by its magic). `hz` 0: not filled in, every clock answers -ENOSYS.
 */
__attribute__((used, aligned(8))) static volatile const struct {
	u64 magic;
	u64 base;
	u64 hz;
} myos_clock = { 0x6b6c63736f796dULL /* "myosclk" */, 0, 0 };

static inline u64 counter(void)
{
#if defined(__x86_64__)
	unsigned lo, hi;
	__asm__ volatile("lfence; rdtsc" : "=a"(lo), "=d"(hi));
	return (u64)hi << 32 | lo;
#elif defined(__aarch64__)
	u64 c;
	__asm__ volatile("isb; mrs %0, cntvct_el0" : "=r"(c));
	return c;
#elif defined(__riscv)
	u64 t;
	__asm__ volatile("rdtime %0" : "=r"(t));
	return t;
#else
#error "no counter for this arch"
#endif
}

/* The layer's monotonic clocks (`sys::clock`): MONOTONIC, MONOTONIC_RAW,
 * MONOTONIC_COARSE, BOOTTIME, BOOTTIME_ALARM. */
static inline int monotonic(int clk)
{
	return clk == 1 || clk == 4 || clk == 6 || clk == 7 || clk == 9;
}

__attribute__((visibility("hidden"))) int vdso_clock_gettime(int clk, struct timespec *ts)
{
	u64 hz = myos_clock.hz;
	if (!monotonic(clk) || hz == 0)
		return -ENOSYS;
	u64 c = counter() - myos_clock.base;
	/* As `counter_to_ns`: whole seconds, then the remainder's nanoseconds
	 * rounded down (the module maps this only for `hz` under 2^64 / 10^9,
	 * where the product cannot overflow). */
	ts->tv_sec = (long)(c / hz);
	ts->tv_nsec = (long)((c % hz) * NSEC_PER_SEC / hz);
	return 0;
}

/* The name libc looks for, in the version Linux gives it on the arch. */
#if defined(__aarch64__)
int __kernel_clock_gettime(int, struct timespec *) __attribute__((alias("vdso_clock_gettime")));
#else
int __vdso_clock_gettime(int, struct timespec *) __attribute__((alias("vdso_clock_gettime")));
#endif
