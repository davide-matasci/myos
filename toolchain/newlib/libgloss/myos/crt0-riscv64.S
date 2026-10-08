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
    // The destructors (.fini_array) run at exit, after the atexit handlers
    // main registers; the constructors (.init_array) run now, with environ
    // set up.
    lla a0, __libc_fini_array
    call atexit
    call __libc_init_array
    mv a0, s0
    mv a1, s1
    call main
    // main's return goes through newlib's exit (atexit handlers, the
    // stdio flush a file on stdout needs); it does not return, the raw
    // syscall is a guard. a0 already holds the status.
    call exit
    li a7, 1
    ecall

    .globl myos_init_environ
    .globl myos_libc_init
    .extern main
    .extern exit
    .extern atexit
    .extern __libc_init_array
    .extern __libc_fini_array
