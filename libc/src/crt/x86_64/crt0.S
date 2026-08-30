    .text
    .globl _start
_start:
    movq (%rsp), %rdi
    leaq 8(%rsp), %rsi
    call myos_init_environ
    call main
    movl %eax, %edi
    movl $1, %eax
    syscall

    .globl myos_init_environ
    .extern main
