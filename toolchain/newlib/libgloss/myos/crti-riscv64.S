    .section .init,"ax"
    .globl _init
_init:
    addi sp, sp, -16
    sd ra, 8(sp)
    sd s0, 0(sp)
    addi s0, sp, 16

    .section .fini,"ax"
    .globl _fini
_fini:
    addi sp, sp, -16
    sd ra, 8(sp)
    sd s0, 0(sp)
    addi s0, sp, 16
