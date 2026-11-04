fn main() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    // 4 KiB PT_LOAD spacing so the in-memory image stays small enough
    // for the kernel heap (AArch64 lld otherwise uses 64 KiB pages).
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=max-page-size=4096");

    // Keep `module_init` / `module_exit` from `--gc-sections`. Do not use
    // `--export-dynamic`: that exports all of libcore and the image jumps
    // to hundreds of KiB.
    println!("cargo:rustc-link-arg=-u");
    println!("cargo:rustc-link-arg=module_init");
    println!("cargo:rustc-link-arg=-u");
    println!("cargo:rustc-link-arg=module_exit");

    if arch == "x86_64" {
        // x86_64-unknown-none already prefers PIE; be explicit.
        println!("cargo:rustc-link-arg=-pie");
        println!("cargo:rustc-link-arg=-nostdlib");
    }
    // aarch64/riscv64: kernel/build.rs links a PIE too (`-pie -z notext`,
    // since the prebuilt libcore is not PIC); the loader applies the
    // RELATIVE relocs.

    build_vdso(&arch);
}

/// The vDSO for `arch` (`vdso/`, docs/linux-compat.md "vDSO"), as
/// `$OUT_DIR/vdso.so`: one page `vdso.rs` copies into each Linux process.
/// Its clock_gettime goes by the name and the version Linux's vDSO gives
/// it on the arch, which is what musl looks up.
fn build_vdso(arch: &str) {
    use std::path::PathBuf;
    use std::process::Command;
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("vdso");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let (triple, flags, symbol, version): (&str, &[&str], &str, &str) = match arch {
        "x86_64" => ("x86_64-unknown-linux-gnu", &["-mgeneral-regs-only"], "__vdso_clock_gettime", "LINUX_2.6"),
        "aarch64" => ("aarch64-unknown-linux-gnu", &["-mgeneral-regs-only"], "__kernel_clock_gettime", "LINUX_2.6.39"),
        "riscv64" => (
            "riscv64-unknown-linux-gnu",
            &["-march=rv64imac", "-mabi=lp64", "-mno-relax"],
            "__vdso_clock_gettime",
            "LINUX_4.15",
        ),
        other => panic!("vdso: no target for {other}"),
    };
    let script = out.join("vdso.ver");
    std::fs::write(&script, format!("{version} {{\n\tglobal: {symbol};\n\tlocal: *;\n}};\n")).unwrap();
    let so = out.join("vdso.so");
    let status = Command::new(std::env::var("CLANG").unwrap_or_else(|_| "clang".into()))
        .arg(format!("--target={triple}"))
        .args(flags)
        .args(["-O2", "-fPIC", "-ffreestanding", "-fno-stack-protector"])
        .args(["-fno-asynchronous-unwind-tables", "-fno-unwind-tables"])
        .args(["-nostdlib", "-shared", "-fuse-ld=lld"])
        .arg(format!("-Wl,-T,{}", dir.join("vdso.lds").display()))
        .arg(format!("-Wl,--version-script={}", script.display()))
        .args(["-Wl,--hash-style=sysv", "-Wl,-soname=linux-vdso.so.1", "-Wl,--no-undefined"])
        .args(["-Wl,-z,max-page-size=4096", "-Wl,--build-id=none"])
        .arg("-o")
        .arg(&so)
        .arg(dir.join("vdso.c"))
        .status()
        .expect("vdso: clang (with lld) is needed to build the vDSO");
    assert!(status.success(), "vdso: clang failed");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vdso/vdso.c");
    println!("cargo:rerun-if-changed=vdso/vdso.lds");
    println!("cargo:rerun-if-env-changed=CLANG");
}
