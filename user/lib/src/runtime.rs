#[cfg(target_arch = "x86_64")]
pub fn start(main: fn() -> !) -> ! {
    main()
}

#[cfg(target_arch = "aarch64")]
pub fn start(argc: usize, argv: *const usize, main: fn() -> !) -> ! {
    unsafe { crate::args::init_from_regs(argc, argv) };
    main()
}
