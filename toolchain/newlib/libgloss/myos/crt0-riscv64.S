    .text
    .globl _start
_start:
    // Kernel passes argc in a0 and argv in a1 across sret.
    // myos_init_environ walks argv to find envp; a0/a1 are caller-saved.
    mv s0, a0
    mv s1, a1
    mv a0, s0
    mv a1, s1
    call myos_init_environ
    mv a0, s0
    mv a1, s1
    call main
    li a7, 1
    ecall

    .globl myos_init_environ
    .globl myos_libc_init
    .extern main
