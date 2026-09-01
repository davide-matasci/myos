    .text
    .globl _start
_start:
    // Kernel passes argc in x0 and argv in x1 across eret.
    // myos_init_environ walks argv to find envp; x0/x1 are caller-saved.
    mov x19, x0
    mov x20, x1
    bl myos_init_environ
    mov x0, x19
    mov x1, x20
    bl main
    mov x8, #1
    svc #0

    .globl myos_init_environ
    .globl myos_libc_init
    .extern main
