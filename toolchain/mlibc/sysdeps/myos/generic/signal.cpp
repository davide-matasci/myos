// Signals: mlibc's struct sigaction and sigset_t (1024 bits) to the
// kernel's records and 32-bit masks (libmyos/sigtramp.c), and the handler
// dispatch the trampoline calls with the kernel's frame: the siginfo it
// holds (newlib's layout: int si_signo; int si_code; union sigval) to
// mlibc's siginfo_t.
#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>

#include <bits/ensure.h>
#include <mlibc/all-sysdeps.hpp>

#include "libmyos.h"

namespace {

constexpr int FRAME_SIGNO = 1;
constexpr int FRAME_HANDLER = 2;
constexpr int FRAME_SIGINFO = 10;

// mlibc keeps signal `sig` at bit sig - 1 of its sigset_t; the kernel (and
// newlib) at bit sig of a 32-bit mask.
unsigned int mask_of(const sigset_t *set) {
	return set ? (unsigned int)(set->__sig[0] << 1) : 0;
}

void set_mask(sigset_t *set, unsigned int mask) {
	memset(set, 0, sizeof *set);
	set->__sig[0] = mask >> 1;
}

} // namespace

extern "C" void __myos_sigdispatch(unsigned long *frame) {
	siginfo_t info;
	memset(&info, 0, sizeof info);
	info.si_signo = (int)(frame[FRAME_SIGINFO] & 0xffffffff);
	info.si_code = (int)(frame[FRAME_SIGINFO] >> 32);
	info.si_value.sival_ptr = (void *)frame[FRAME_SIGINFO + 1];
	// The kernel keeps no sender. SIGCANCEL has one sender on myos, mlibc's
	// pthread_cancel in this process (Tgkill), and its handler takes the
	// signal only from there: what Linux's tgkill would say.
	if (info.si_signo == SIGCANCEL) {
		info.si_code = SI_TKILL;
		info.si_pid = getpid();
	}
	__myos_sig_count++;
	// A plain sa_handler takes one argument; passing three is harmless.
	auto handler = (void (*)(int, siginfo_t *, void *))(uintptr_t)frame[FRAME_HANDLER];
	handler((int)frame[FRAME_SIGNO], &info, nullptr);
}

namespace mlibc {

int Sysdeps<Sigaction>::operator()(int sig, const struct sigaction *__restrict act,
		struct sigaction *__restrict oact) {
	struct myos_ksigaction kin = {};
	struct myos_ksigaction kout = {};
	if (act != nullptr) {
		kin.handler = (unsigned long)(uintptr_t)act->sa_handler;
		kin.flags = (unsigned long)(unsigned int)(act->sa_flags & ~SA_RESTORER);
		kin.mask = mask_of(&act->sa_mask);
	}
	if (__myos_sigaction_k(sig, act ? &kin : nullptr, oact ? &kout : nullptr) != 0)
		return errno;
	if (oact != nullptr) {
		memset(oact, 0, sizeof *oact);
		oact->sa_handler = (void (*)(int))(uintptr_t)kout.handler;
		oact->sa_flags = (int)kout.flags;
		set_mask(&oact->sa_mask, (unsigned int)kout.mask);
	}
	return 0;
}

// The blocked mask is each thread's (docs/threads.md).
int Sysdeps<Sigprocmask>::operator()(int how, const sigset_t *__restrict set, sigset_t *__restrict retrieve) {
	unsigned int kset = mask_of(set);
	unsigned int kold = 0;
	if (__myos_sigprocmask_k(how, set ? &kset : nullptr, retrieve ? &kold : nullptr) != 0)
		return errno;
	if (retrieve != nullptr)
		set_mask(retrieve, kold);
	return 0;
}

int Sysdeps<ThreadSigmask>::operator()(int how, const sigset_t *__restrict set, sigset_t *__restrict retrieve) {
	return sysdep<Sigprocmask>(how, set, retrieve);
}

int Sysdeps<Sigpending>::operator()(sigset_t *set) {
	set_mask(set, __myos_sigpending_k());
	return 0;
}

int Sysdeps<Sigsuspend>::operator()(const sigset_t *set) {
	__myos_sigsuspend_k(mask_of(set));
	return EINTR;
}

int Sysdeps<Pause>::operator()() {
	unsigned int cur = 0;
	if (__myos_sigprocmask_k(SIG_BLOCK, nullptr, &cur) != 0)
		return errno;
	__myos_sigsuspend_k(cur);
	return EINTR;
}

// sigwait: the kernel has no timed wait for a signal.
int Sysdeps<Sigtimedwait>::operator()(const sigset_t *__restrict set, siginfo_t *__restrict info,
		const struct timespec *__restrict timeout, int *out_signal) {
	if (timeout != nullptr)
		return ENOSYS;
	int sig = __myos_sigwait_k(mask_of(set));
	if (sig < 0)
		return errno;
	if (info != nullptr) {
		memset(info, 0, sizeof *info);
		info->si_signo = sig;
		info->si_code = SI_USER;
	}
	*out_signal = sig;
	return 0;
}

int Sysdeps<Kill>::operator()(pid_t pid, int sig) {
	return __myos_kill(pid, sig) < 0 ? errno : 0;
}

// A signal sent to a tid goes to that thread (docs/threads.md).
int Sysdeps<Tgkill>::operator()(int tgid, int tid, int sig) {
	(void)tgid;
	return __myos_kill(tid, sig) < 0 ? errno : 0;
}

} // namespace mlibc
