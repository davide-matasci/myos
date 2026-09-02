    .section .init,"ax",@progbits
    .globl _init
_init:
    pushq %rbp
    movq %rsp, %rbp

    .section .fini,"ax",@progbits
    .globl _fini
_fini:
    pushq %rbp
    movq %rsp, %rbp
