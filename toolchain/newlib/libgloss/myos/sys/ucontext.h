/* The interrupted context a signal handler is given as its third argument
 * (docs/signals.md "Delivery"): the registers the trampoline saved
 * (toolchain/libmyos/sigtramp.c), which it loads back when the handler
 * returns, changed or not. The layouts and names are the ones programs
 * written for Linux expect (and mlibc's, toolchain/mlibc/sysdeps/myos:
 * one conversion serves both libcs). uc_stack says no alternate stack
 * (there is none), uc_link is NULL; getcontext and its family are not
 * provided. */
#ifndef _SYS_UCONTEXT_H
#define _SYS_UCONTEXT_H

#include <signal.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#if defined(__x86_64__)

#define REG_R8 0
#define REG_R9 1
#define REG_R10 2
#define REG_R11 3
#define REG_R12 4
#define REG_R13 5
#define REG_R14 6
#define REG_R15 7
#define REG_RDI 8
#define REG_RSI 9
#define REG_RBP 10
#define REG_RBX 11
#define REG_RDX 12
#define REG_RAX 13
#define REG_RCX 14
#define REG_RSP 15
#define REG_RIP 16
#define REG_EFL 17
#define NGREG 23

typedef unsigned long greg_t, gregset_t[NGREG];

/* The FXSAVE image: the x87 and SSE registers. */
struct _fpstate {
    uint16_t cwd, swd, ftw, fop;
    uint64_t rip, rdp;
    uint32_t mxcsr, mxcr_mask;
    struct {
        uint16_t significand[4], exponent, __padding[3];
    } _st[8];
    struct {
        uint32_t element[4];
    } _xmm[16];
    uint32_t __padding[24];
};

typedef struct {
    gregset_t gregs;
    struct _fpstate *fpregs;
    unsigned long __reserved1[8];
} mcontext_t;

typedef struct __ucontext {
    unsigned long uc_flags;
    struct __ucontext *uc_link;
    stack_t uc_stack;
    mcontext_t uc_mcontext;
    sigset_t uc_sigmask;
} ucontext_t;

#elif defined(__aarch64__)

#define NGREG 34
#define FPSIMD_MAGIC 0x46508001

struct _aarch64_ctx {
    uint32_t magic, size;
};

/* What __reserved starts with: the FP/SIMD registers. */
struct fpsimd_context {
    struct _aarch64_ctx head;
    uint32_t fpsr, fpcr;
    __uint128_t vregs[32];
};

typedef struct sigcontext {
    uint64_t fault_address;
    uint64_t regs[31];
    uint64_t sp, pc, pstate;
    uint8_t __reserved[4096] __attribute__((__aligned__(16)));
} mcontext_t;

typedef struct __ucontext {
    unsigned long uc_flags;
    struct __ucontext *uc_link;
    stack_t uc_stack;
    sigset_t uc_sigmask;
    uint8_t __uc_pad[1024 / 8 - sizeof(sigset_t)];
    mcontext_t uc_mcontext;
} ucontext_t;

#elif defined(__riscv) && __riscv_xlen == 64

#define NGREG 32
#define REG_PC 0
#define REG_RA 1
#define REG_SP 2
#define REG_TP 4
#define REG_S0 8
#define REG_A0 10

struct __riscv_d_ext_state {
    uint64_t f[32];
    uint32_t fcsr;
};

union __riscv_fp_state {
    struct __riscv_d_ext_state d;
    uint64_t __q[66] __attribute__((__aligned__(16)));
};

/* __gregs[0] is the pc, __gregs[n] the register xn (glibc's and musl's
 * member names). */
typedef struct sigcontext {
    unsigned long __gregs[NGREG];
    union __riscv_fp_state __fpregs;
} mcontext_t;

typedef struct __ucontext {
    unsigned long uc_flags;
    struct __ucontext *uc_link;
    stack_t uc_stack;
    sigset_t uc_sigmask;
    uint8_t __uc_pad[1024 / 8 - sizeof(sigset_t)];
    mcontext_t uc_mcontext;
} ucontext_t;

#else
#error unsupported arch
#endif

#ifdef __cplusplus
}
#endif

#endif
