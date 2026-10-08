/* newlib's __libc_init_array and __libc_fini_array, which crt0 runs, call
 * _init and _fini where configure.host sets _HAVE_INIT_FINI (aarch64's
 * arm does, riscv64's does not). They are crti.o's and crtn.o's, which only
 * tcc's links carry: the other links are crt0.o, libc and libgloss, and
 * their constructors are in .init_array, with nothing in .init. These empty
 * ones stand in; weak, so crti.o's win where it is linked. */
__attribute__((weak)) void _init(void) {
}

__attribute__((weak)) void _fini(void) {
}
