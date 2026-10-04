    .text
    .globl _start
_start:
    movq (%rsp), %rdi
    leaq 8(%rsp), %rsi
    // myos_init_environ walks argv to find envp; rdi/rsi are caller-saved.
    movq %rdi, %r12
    movq %rsi, %r13
    call myos_init_environ
    movq %r12, %rdi
    movq %r13, %rsi
    call main
    // main's return goes through newlib's exit: the atexit handlers and
    // the stdio flush, which a file or pipe on stdout needs (a terminal
    // is line buffered). exit does not return; the raw syscall is a guard.
    movl %eax, %edi
    call exit
    movl $1, %eax
    syscall

    .globl myos_init_environ
    .globl myos_libc_init
    .extern main
    .extern exit
