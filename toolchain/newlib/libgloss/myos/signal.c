/*
 * Signal handlers on myos: sigaction(), signal() and the trampoline the
 * kernel enters to run a caught signal's handler.
 *
 * The kernel delivers on the way out of a syscall. It writes a frame below
 * the interrupted stack pointer (see kernel/src/signal.rs) and resumes here,
 * at __myos_sigtramp, with the stack pointer at that frame and every other
 * register as the interrupted code left it (the result register aside: the
 * frame holds the syscall's result). The trampoline saves all integer and
 * FP registers, calls the handler through __myos_sigdispatch, restores them
 * and calls SYS_SIGRETURN with the stack pointer back at the frame; the
 * kernel then restores the signal mask and resumes the interrupted code.
 *
 * Frame words: [0] magic, [1] signo, [2] handler, [3] saved mask, [4] pc,
 * [5] sp, [6] result, [7] number register, [10..13] siginfo_t.
 */

#include <errno.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "myos_syscalls.h"

#define MYOS_SIGFRAME_SIGNO 1
#define MYOS_SIGFRAME_HANDLER 2
#define MYOS_SIGFRAME_SIGINFO 10

/* Number of handlers run so far. select()/poll()/nanosleep() wait in
 * userspace loops, so they compare it before and after to return EINTR when
 * a handler ran meanwhile. */
volatile unsigned long __myos_sig_count;

void __myos_sigdispatch(unsigned long *frame);

void __myos_sigdispatch(unsigned long *frame) {
    void (*handler)(int, siginfo_t *, void *) =
        (void (*)(int, siginfo_t *, void *))(uintptr_t)frame[MYOS_SIGFRAME_HANDLER];
    __myos_sig_count++;
    /* A plain sa_handler takes one argument; passing three is harmless. */
    handler((int)frame[MYOS_SIGFRAME_SIGNO],
            (siginfo_t *)&frame[MYOS_SIGFRAME_SIGINFO], NULL);
}

void __myos_sigtramp(void);

#if defined(__x86_64__)
/* Save area: rax..r15 (15 words, rsp is the frame) + FXSAVE (512 bytes,
 * 16-aligned). 640 bytes keeps rsp 16-aligned for the call. rcx/r11 are
 * clobbered by the final `syscall`, as by the interrupted one. */
__asm__(
    ".pushsection .text\n"
    ".globl __myos_sigtramp\n"
    ".type __myos_sigtramp,@function\n"
    "__myos_sigtramp:\n"
    "    sub $640, %rsp\n"
    "    mov %rax, 0(%rsp)\n"
    "    mov %rbx, 8(%rsp)\n"
    "    mov %rcx, 16(%rsp)\n"
    "    mov %rdx, 24(%rsp)\n"
    "    mov %rsi, 32(%rsp)\n"
    "    mov %rdi, 40(%rsp)\n"
    "    mov %rbp, 48(%rsp)\n"
    "    mov %r8, 56(%rsp)\n"
    "    mov %r9, 64(%rsp)\n"
    "    mov %r10, 72(%rsp)\n"
    "    mov %r11, 80(%rsp)\n"
    "    mov %r12, 88(%rsp)\n"
    "    mov %r13, 96(%rsp)\n"
    "    mov %r14, 104(%rsp)\n"
    "    mov %r15, 112(%rsp)\n"
    "    fxsave64 128(%rsp)\n"
    "    cld\n"
    "    lea 640(%rsp), %rdi\n"
    "    call __myos_sigdispatch\n"
    "    fxrstor64 128(%rsp)\n"
    "    mov 0(%rsp), %rax\n"
    "    mov 8(%rsp), %rbx\n"
    "    mov 16(%rsp), %rcx\n"
    "    mov 24(%rsp), %rdx\n"
    "    mov 32(%rsp), %rsi\n"
    "    mov 40(%rsp), %rdi\n"
    "    mov 48(%rsp), %rbp\n"
    "    mov 56(%rsp), %r8\n"
    "    mov 64(%rsp), %r9\n"
    "    mov 72(%rsp), %r10\n"
    "    mov 80(%rsp), %r11\n"
    "    mov 88(%rsp), %r12\n"
    "    mov 96(%rsp), %r13\n"
    "    mov 104(%rsp), %r14\n"
    "    mov 112(%rsp), %r15\n"
    "    add $640, %rsp\n"
    "    mov $" MYOS_STR(MYOS_SYS_SIGRETURN) ", %eax\n"
    "    syscall\n"
    "    ud2\n"
    ".size __myos_sigtramp, .-__myos_sigtramp\n"
    ".popsection\n");
#elif defined(__aarch64__)
/* Save area: x0..x30 + NZCV (32 words), q0..q31 (512 bytes), FPCR/FPSR:
 * 784 bytes, a multiple of 16. x8 carries SYS_SIGRETURN; the kernel puts
 * back the interrupted x8 from the frame. */
__asm__(
    ".pushsection .text\n"
    ".globl __myos_sigtramp\n"
    ".type __myos_sigtramp,%function\n"
    "__myos_sigtramp:\n"
    "    sub sp, sp, #784\n"
    "    stp x0, x1, [sp, #0]\n"
    "    stp x2, x3, [sp, #16]\n"
    "    stp x4, x5, [sp, #32]\n"
    "    stp x6, x7, [sp, #48]\n"
    "    stp x8, x9, [sp, #64]\n"
    "    stp x10, x11, [sp, #80]\n"
    "    stp x12, x13, [sp, #96]\n"
    "    stp x14, x15, [sp, #112]\n"
    "    stp x16, x17, [sp, #128]\n"
    "    stp x18, x19, [sp, #144]\n"
    "    stp x20, x21, [sp, #160]\n"
    "    stp x22, x23, [sp, #176]\n"
    "    stp x24, x25, [sp, #192]\n"
    "    stp x26, x27, [sp, #208]\n"
    "    stp x28, x29, [sp, #224]\n"
    "    mrs x9, nzcv\n"
    "    stp x30, x9, [sp, #240]\n"
    "    stp q0, q1, [sp, #256]\n"
    "    stp q2, q3, [sp, #288]\n"
    "    stp q4, q5, [sp, #320]\n"
    "    stp q6, q7, [sp, #352]\n"
    "    stp q8, q9, [sp, #384]\n"
    "    stp q10, q11, [sp, #416]\n"
    "    stp q12, q13, [sp, #448]\n"
    "    stp q14, q15, [sp, #480]\n"
    "    stp q16, q17, [sp, #512]\n"
    "    stp q18, q19, [sp, #544]\n"
    "    stp q20, q21, [sp, #576]\n"
    "    stp q22, q23, [sp, #608]\n"
    "    stp q24, q25, [sp, #640]\n"
    "    stp q26, q27, [sp, #672]\n"
    "    stp q28, q29, [sp, #704]\n"
    "    stp q30, q31, [sp, #736]\n"
    "    mrs x9, fpcr\n"
    "    mrs x10, fpsr\n"
    "    add x11, sp, #768\n"
    "    stp x9, x10, [x11]\n"
    "    add x0, sp, #784\n"
    "    bl __myos_sigdispatch\n"
    "    add x11, sp, #768\n"
    "    ldp x9, x10, [x11]\n"
    "    msr fpcr, x9\n"
    "    msr fpsr, x10\n"
    "    ldp q0, q1, [sp, #256]\n"
    "    ldp q2, q3, [sp, #288]\n"
    "    ldp q4, q5, [sp, #320]\n"
    "    ldp q6, q7, [sp, #352]\n"
    "    ldp q8, q9, [sp, #384]\n"
    "    ldp q10, q11, [sp, #416]\n"
    "    ldp q12, q13, [sp, #448]\n"
    "    ldp q14, q15, [sp, #480]\n"
    "    ldp q16, q17, [sp, #512]\n"
    "    ldp q18, q19, [sp, #544]\n"
    "    ldp q20, q21, [sp, #576]\n"
    "    ldp q22, q23, [sp, #608]\n"
    "    ldp q24, q25, [sp, #640]\n"
    "    ldp q26, q27, [sp, #672]\n"
    "    ldp q28, q29, [sp, #704]\n"
    "    ldp q30, q31, [sp, #736]\n"
    "    ldp x30, x9, [sp, #240]\n"
    "    msr nzcv, x9\n"
    "    ldp x0, x1, [sp, #0]\n"
    "    ldp x2, x3, [sp, #16]\n"
    "    ldp x4, x5, [sp, #32]\n"
    "    ldp x6, x7, [sp, #48]\n"
    "    ldp x8, x9, [sp, #64]\n"
    "    ldp x10, x11, [sp, #80]\n"
    "    ldp x12, x13, [sp, #96]\n"
    "    ldp x14, x15, [sp, #112]\n"
    "    ldp x16, x17, [sp, #128]\n"
    "    ldp x18, x19, [sp, #144]\n"
    "    ldp x20, x21, [sp, #160]\n"
    "    ldp x22, x23, [sp, #176]\n"
    "    ldp x24, x25, [sp, #192]\n"
    "    ldp x26, x27, [sp, #208]\n"
    "    ldp x28, x29, [sp, #224]\n"
    "    add sp, sp, #784\n"
    "    mov x8, #" MYOS_STR(MYOS_SYS_SIGRETURN) "\n"
    "    svc #0\n"
    "    brk #0\n"
    ".size __myos_sigtramp, .-__myos_sigtramp\n"
    ".popsection\n");
#elif defined(__riscv) && __riscv_xlen == 64
/* Save area: x1, x3..x31 at their register number (x0/x2 slots unused),
 * then f0..f31 + fcsr on hard-float builds. a7 carries SYS_SIGRETURN; the
 * kernel puts back the interrupted a7 from the frame. */
#if defined(__riscv_flen) && __riscv_flen == 64
#define MYOS_RV_SAVE 528
#define MYOS_RV_FP_SAVE \
    "    fsd f0, 256(sp)\n    fsd f1, 264(sp)\n    fsd f2, 272(sp)\n    fsd f3, 280(sp)\n" \
    "    fsd f4, 288(sp)\n    fsd f5, 296(sp)\n    fsd f6, 304(sp)\n    fsd f7, 312(sp)\n" \
    "    fsd f8, 320(sp)\n    fsd f9, 328(sp)\n    fsd f10, 336(sp)\n    fsd f11, 344(sp)\n" \
    "    fsd f12, 352(sp)\n    fsd f13, 360(sp)\n    fsd f14, 368(sp)\n    fsd f15, 376(sp)\n" \
    "    fsd f16, 384(sp)\n    fsd f17, 392(sp)\n    fsd f18, 400(sp)\n    fsd f19, 408(sp)\n" \
    "    fsd f20, 416(sp)\n    fsd f21, 424(sp)\n    fsd f22, 432(sp)\n    fsd f23, 440(sp)\n" \
    "    fsd f24, 448(sp)\n    fsd f25, 456(sp)\n    fsd f26, 464(sp)\n    fsd f27, 472(sp)\n" \
    "    fsd f28, 480(sp)\n    fsd f29, 488(sp)\n    fsd f30, 496(sp)\n    fsd f31, 504(sp)\n" \
    "    frcsr t0\n    sd t0, 512(sp)\n"
#define MYOS_RV_FP_LOAD \
    "    ld t0, 512(sp)\n    fscsr t0\n" \
    "    fld f0, 256(sp)\n    fld f1, 264(sp)\n    fld f2, 272(sp)\n    fld f3, 280(sp)\n" \
    "    fld f4, 288(sp)\n    fld f5, 296(sp)\n    fld f6, 304(sp)\n    fld f7, 312(sp)\n" \
    "    fld f8, 320(sp)\n    fld f9, 328(sp)\n    fld f10, 336(sp)\n    fld f11, 344(sp)\n" \
    "    fld f12, 352(sp)\n    fld f13, 360(sp)\n    fld f14, 368(sp)\n    fld f15, 376(sp)\n" \
    "    fld f16, 384(sp)\n    fld f17, 392(sp)\n    fld f18, 400(sp)\n    fld f19, 408(sp)\n" \
    "    fld f20, 416(sp)\n    fld f21, 424(sp)\n    fld f22, 432(sp)\n    fld f23, 440(sp)\n" \
    "    fld f24, 448(sp)\n    fld f25, 456(sp)\n    fld f26, 464(sp)\n    fld f27, 472(sp)\n" \
    "    fld f28, 480(sp)\n    fld f29, 488(sp)\n    fld f30, 496(sp)\n    fld f31, 504(sp)\n"
#else
#define MYOS_RV_SAVE 256
#define MYOS_RV_FP_SAVE ""
#define MYOS_RV_FP_LOAD ""
#endif
__asm__(
    ".pushsection .text\n"
    ".globl __myos_sigtramp\n"
    ".type __myos_sigtramp,@function\n"
    "__myos_sigtramp:\n"
    "    addi sp, sp, -" MYOS_STR(MYOS_RV_SAVE) "\n"
    "    sd x1, 8(sp)\n"
    "    sd x3, 24(sp)\n"
    "    sd x4, 32(sp)\n"
    "    sd x5, 40(sp)\n"
    "    sd x6, 48(sp)\n"
    "    sd x7, 56(sp)\n"
    "    sd x8, 64(sp)\n"
    "    sd x9, 72(sp)\n"
    "    sd x10, 80(sp)\n"
    "    sd x11, 88(sp)\n"
    "    sd x12, 96(sp)\n"
    "    sd x13, 104(sp)\n"
    "    sd x14, 112(sp)\n"
    "    sd x15, 120(sp)\n"
    "    sd x16, 128(sp)\n"
    "    sd x17, 136(sp)\n"
    "    sd x18, 144(sp)\n"
    "    sd x19, 152(sp)\n"
    "    sd x20, 160(sp)\n"
    "    sd x21, 168(sp)\n"
    "    sd x22, 176(sp)\n"
    "    sd x23, 184(sp)\n"
    "    sd x24, 192(sp)\n"
    "    sd x25, 200(sp)\n"
    "    sd x26, 208(sp)\n"
    "    sd x27, 216(sp)\n"
    "    sd x28, 224(sp)\n"
    "    sd x29, 232(sp)\n"
    "    sd x30, 240(sp)\n"
    "    sd x31, 248(sp)\n"
    MYOS_RV_FP_SAVE
    "    addi a0, sp, " MYOS_STR(MYOS_RV_SAVE) "\n"
    "    call __myos_sigdispatch\n"
    MYOS_RV_FP_LOAD
    "    ld x1, 8(sp)\n"
    "    ld x3, 24(sp)\n"
    "    ld x4, 32(sp)\n"
    "    ld x5, 40(sp)\n"
    "    ld x6, 48(sp)\n"
    "    ld x7, 56(sp)\n"
    "    ld x8, 64(sp)\n"
    "    ld x9, 72(sp)\n"
    "    ld x10, 80(sp)\n"
    "    ld x11, 88(sp)\n"
    "    ld x12, 96(sp)\n"
    "    ld x13, 104(sp)\n"
    "    ld x14, 112(sp)\n"
    "    ld x15, 120(sp)\n"
    "    ld x16, 128(sp)\n"
    "    ld x17, 136(sp)\n"
    "    ld x18, 144(sp)\n"
    "    ld x19, 152(sp)\n"
    "    ld x20, 160(sp)\n"
    "    ld x21, 168(sp)\n"
    "    ld x22, 176(sp)\n"
    "    ld x23, 184(sp)\n"
    "    ld x24, 192(sp)\n"
    "    ld x25, 200(sp)\n"
    "    ld x26, 208(sp)\n"
    "    ld x27, 216(sp)\n"
    "    ld x28, 224(sp)\n"
    "    ld x29, 232(sp)\n"
    "    ld x30, 240(sp)\n"
    "    ld x31, 248(sp)\n"
    "    addi sp, sp, " MYOS_STR(MYOS_RV_SAVE) "\n"
    "    li a7, " MYOS_STR(MYOS_SYS_SIGRETURN) "\n"
    "    ecall\n"
    "    unimp\n"
    ".size __myos_sigtramp, .-__myos_sigtramp\n"
    ".popsection\n");
#else
#error unsupported arch
#endif

/* Kernel ABI: {handler, flags, mask, trampoline}; handler 0 = SIG_DFL,
 * 1 = SIG_IGN, else a function run through the trampoline. The kernel
 * reports only the first three words back. */
struct myos_ksigaction {
    unsigned long handler;
    unsigned long flags;
    unsigned long mask;
    unsigned long tramp;
};

int sigaction(int sig, const struct sigaction *restrict act,
    struct sigaction *restrict oact) {
    struct myos_ksigaction kin;
    struct myos_ksigaction kout;
    long ret;

    if (sig <= 0 || sig > 31) {
        errno = EINVAL;
        return -1;
    }
    memset(&kin, 0, sizeof(kin));
    memset(&kout, 0, sizeof(kout));
    if (act != NULL) {
        kin.handler = (unsigned long)(uintptr_t)act->sa_handler;
        kin.flags = (unsigned long)(unsigned int)act->sa_flags;
        kin.mask = (unsigned long)act->sa_mask;
        kin.tramp = (unsigned long)(uintptr_t)__myos_sigtramp;
    }
    ret = myos_syscall3(
        MYOS_SYS_SIGACTION2,
        (long)sig,
        act ? (long)(uintptr_t)&kin : 0,
        oact ? (long)(uintptr_t)&kout : 0);
    if (ret == (long)MYOS_SYSERR) {
        /* SIGKILL/SIGSTOP cannot be caught or ignored. */
        errno = EINVAL;
        return -1;
    }
    if (oact != NULL) {
        memset(oact, 0, sizeof(*oact));
        oact->sa_handler = (_sig_func_ptr)(uintptr_t)kout.handler;
        oact->sa_flags = (int)kout.flags;
        oact->sa_mask = (sigset_t)kout.mask;
    }
    return 0;
}

/* BSD semantics, like glibc: the handler stays installed and interrupted
 * syscalls restart. */
_sig_func_ptr signal(int sig, _sig_func_ptr func) {
    struct sigaction sa;
    struct sigaction old;

    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = func;
    sa.sa_flags = SA_RESTART;
    if (sigaction(sig, &sa, &old) != 0) {
        return SIG_ERR;
    }
    return old.sa_handler;
}

int sigpending(sigset_t *set) {
    if (set == NULL) {
        errno = EFAULT;
        return -1;
    }
    *set = (sigset_t)myos_syscall0(MYOS_SYS_SIGPENDING);
    return 0;
}

/* Waits with `mask` blocked until a handler has run (or the task dies);
 * the kernel puts the previous mask back. Always -1/EINTR. */
int sigsuspend(const sigset_t *mask) {
    if (mask == NULL) {
        errno = EFAULT;
        return -1;
    }
    __myos_cancel_enter();
    (void)myos_syscall1(MYOS_SYS_SIGSUSPEND, (long)*mask);
    __myos_cancel_leave();
    errno = EINTR;
    return -1;
}

int sigwait(const sigset_t *restrict set, int *restrict sig) {
    long r;

    if (set == NULL || sig == NULL) {
        return EINVAL;
    }
    /* POSIX: sigwait does not fail with EINTR; retry after other handlers. */
    __myos_cancel_enter();
    do {
        r = myos_syscall1(MYOS_SYS_SIGWAIT, (long)*set);
    } while (r == (long)MYOS_EINTR);
    __myos_cancel_leave();
    if (r <= 0 || r > 31) {
        return EINVAL;
    }
    *sig = (int)r;
    return 0;
}

/* Processes have a single thread, so the thread mask is the process mask. */
int pthread_sigmask(int how, const sigset_t *restrict set, sigset_t *restrict oset) {
    return sigprocmask(how, set, oset) == 0 ? 0 : errno;
}
