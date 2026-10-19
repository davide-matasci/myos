// The first code a Raspberry Pi 3 runs (the firmware's `armstub=`, QEMU's
// `raspi3b` with every core at address 0): every core enters at 0, at EL3
// on a Pi, at EL2 under QEMU, and leaves at EL1, the level Limine boots a
// Cortex-A53 at (it has no VHE: Limine refuses EL2 without it). Core 0
// jumps to the kernel the firmware loaded (U-Boot, at `kernel_entry32`)
// with the device tree's address in x0; the others wait for a spin-table
// release at the address the device tree gives them (`cpu-release-addr`,
// 0xd8 + 8 * core), as Limine's SMP start-up expects.
//
// It stays at EL2 as a minimal PSCI for the kernel's HVC (the tree has
// no `/psci`: the kernel calls at EL1 through HVC, `docs/power.md`):
// `SYSTEM_RESET` and `SYSTEM_OFF` through the power manager's watchdog,
// the latter as a reset the firmware stops at (the reset status register's
// partition 63, which it reads as "halt"); any other call is
// NOT_SUPPORTED. The first page, the stub's, is reserved in the firmware's
// device tree (`/memreserve/`), so it stays in place.
//
// The layout from 0xd8 to 0x100 is the firmware's interface: it finds the
// stub by the magic at 0xf0 and writes the device tree's address and the
// kernel's entry there; it clears the magic afterwards, which leaves 0xf0
// as core 3's release slot.

	.section .text
	.global _start
_start:
	b	entry

	.org 0xd8
spin_cpu0:	.quad 0
	.org 0xe0
spin_cpu1:	.quad 0
	.org 0xe8
spin_cpu2:	.quad 0
	.org 0xf0
spin_cpu3:
stub_magic:	.word 0x5afe570b
	.org 0xf4
stub_version:	.word 0
	.org 0xf8
dtb_ptr32:	.word 0
	.org 0xfc
kernel_entry32:	.word 0

	.org 0x100
entry:
	mrs	x6, mpidr_el1
	and	x6, x6, #3
	mrs	x0, CurrentEL
	lsr	x0, x0, #2
	cmp	x0, #3
	b.ne	at_el2

	// EL3 (a Pi): what its firmware leaves to the stub. The counter's
	// frequency (the 19.2 MHz crystal), the A53's SMP coherency bit
	// (CPUECTLR_EL1.SMPEN) and the lower levels' access to it (ACTLR_EL3:
	// CPUACTLR, CPUECTLR, L2CTLR, L2ECTLR, L2ACTLR).
	ldr	x0, =19200000
	msr	cntfrq_el0, x0
	mov	x0, #0x73
	msr	actlr_el3, x0
	mrs	x0, s3_1_c15_c2_1
	orr	x0, x0, #(1 << 6)
	msr	s3_1_c15_c2_1, x0
	// The local timer's prescaler 1:1, so the generic counter runs at the
	// crystal's rate (core 0, once).
	cbnz	x6, 1f
	mov	x1, #0x40000000
	str	wzr, [x1]
	mov	w0, #0x80000000
	str	w0, [x1, #8]
1:
	// Non-secure lower levels, EL2 in AArch64, HVC on, SMC off.
	mov	x0, #0x5b1
	msr	scr_el3, x0
	// To EL2h, interrupts masked.
	mov	x0, #0x3c9
	msr	spsr_el3, x0
	adr	x0, at_el2
	msr	elr_el3, x0
	eret

at_el2:
	// The physical counter and timer to EL1, no virtual offset.
	mov	x0, #3
	msr	cnthctl_el2, x0
	msr	cntvoff_el2, xzr
	// EL1 in AArch64, seeing this core's own MIDR and MPIDR.
	mov	x0, #(1 << 31)
	msr	hcr_el2, x0
	mrs	x0, midr_el1
	msr	vpidr_el2, x0
	mrs	x0, mpidr_el1
	msr	vmpidr_el2, x0
	// No traps to EL2 (FP/SIMD, CP15).
	mov	x0, #0x33ff
	msr	cptr_el2, x0
	msr	hstr_el2, xzr
	// HVC from EL1 to the PSCI below.
	adr	x0, el2_vectors
	msr	vbar_el2, x0
	// EL1 with its MMU and caches off.
	ldr	x0, =0x30d00800
	msr	sctlr_el1, x0
	// To EL1h, interrupts masked.
	mov	x0, #0x3c5
	msr	spsr_el2, x0
	adr	x0, at_el1
	msr	elr_el2, x0
	eret

at_el1:
	cbz	x6, primary
	adr	x5, spin_cpu0
	ldr	w7, magic
wait:
	wfe
	ldr	x4, [x5, x6, lsl #3]
	cbz	x4, wait
	// Core 3's slot holds the magic until a firmware clears it (QEMU has
	// none).
	cmp	x4, x7
	b.eq	wait
	mov	x0, #0
	mov	x1, #0
	mov	x2, #0
	mov	x3, #0
	br	x4

primary:
	ldr	w4, kernel_entry32
	ldr	w0, dtb_ptr32
	mov	x1, #0
	mov	x2, #0
	mov	x3, #0
	br	x4

// PSCI, from the lower level's HVC: x0 the function, the result in x0.
psci:
	mrs	x9, esr_el2
	lsr	x9, x9, #26
	cmp	x9, #0x16		// HVC from AArch64
	b.ne	.
	ldr	w9, =0x84000008		// SYSTEM_OFF
	cmp	w0, w9
	b.eq	system_off
	ldr	w9, =0x84000009		// SYSTEM_RESET
	cmp	w0, w9
	b.eq	system_reset
	mov	x0, #-1			// NOT_SUPPORTED
	eret

// The power manager (`PM_RSTC`, `PM_RSTS`, `PM_WDOG`, written with its
// password in the top byte).
PM		= 0x3f100000
PM_RSTC		= 0x1c
PM_RSTS		= 0x20
PM_WDOG		= 0x24
PM_PASSWORD	= 0x5a000000

system_off:
	// Boot partition 63, the firmware's "halt": its bits are every other
	// one of the low 12 in RSTS.
	ldr	x10, =PM
	ldr	w11, [x10, #PM_RSTS]
	ldr	w12, =0xfffffaaa
	and	w11, w11, w12
	ldr	w12, =(PM_PASSWORD | 0x555)
	orr	w11, w11, w12
	str	w11, [x10, #PM_RSTS]
system_reset:
	// The watchdog fires in 10 ticks (of 16 us), a full reset.
	ldr	x10, =PM
	ldr	w11, =(PM_PASSWORD | 10)
	str	w11, [x10, #PM_WDOG]
	ldr	w11, [x10, #PM_RSTC]
	and	w11, w11, #~0x30
	ldr	w12, =(PM_PASSWORD | 0x20)
	orr	w11, w11, w12
	str	w11, [x10, #PM_RSTC]
1:	wfe
	b	1b

magic:
	.word	0x5afe570b
	.ltorg

// EL2's vectors: a synchronous exception from AArch64 EL1 (HVC) goes to
// the PSCI above; nothing else is expected (EL1 takes its own interrupts).
	.org 0x800
el2_vectors:
	.rept 8
	b	.
	.balign 0x80
	.endr
	b	psci
	.balign 0x80
	.rept 7
	b	.
	.balign 0x80
	.endr
