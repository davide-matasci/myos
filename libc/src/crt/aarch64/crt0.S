    .text
    .globl _start
_start:
    // Kernel passes argc in x0 and argv in x1 across eret.
    bl myos_init_environ
    bl main
    mov x8, #1
    svc #0

    .globl myos_init_environ
    .extern main
