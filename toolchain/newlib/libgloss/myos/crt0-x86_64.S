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
    movl %eax, %edi
    movl $1, %eax
    syscall

    .globl myos_init_environ
    .globl myos_libc_init
    .extern main
