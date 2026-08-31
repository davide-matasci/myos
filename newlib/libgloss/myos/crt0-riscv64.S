    .text
    .globl _start
_start:
    // Kernel passes argc in a0 and argv in a1 across sret.
    call myos_init_environ
    call main
    li a7, 1
    ecall

    .globl myos_init_environ
    .globl myos_libc_init
    .extern main
