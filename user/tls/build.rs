use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../..");
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if arch.is_empty() {
        return;
    }

    let build_sh = root.join("ports/mbedtls/build.sh");
    println!("cargo:rerun-if-changed={}", build_sh.display());
    println!("cargo:rerun-if-changed={}", root.join("ports/mbedtls").display());

    // Ensure newlib wrappers are on PATH for the port script.
    let newlib_bin = root.join("target/newlib-bin");
    let path = env::var("PATH").unwrap_or_default();
    let path = format!("{}:{}", newlib_bin.display(), path);

    let output = Command::new("bash")
        .arg(&build_sh)
        .current_dir(&root)
        .env("PATH", &path)
        .output()
        .expect("run ports/mbedtls/build.sh");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    print!("{stdout}");
    eprint!("{stderr}");
    if !output.status.success() {
        panic!(
            "ports/mbedtls/build.sh failed (status {:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
            output.status.code()
        );
    }

    let lib = root.join(format!("target/mbedtls-{arch}/lib"));
    let inc = root.join(format!("target/mbedtls-{arch}/include"));
    let newlib_inc = root.join(format!("target/newlib-{arch}/{arch}-unknown-myos/include"));
    let newlib_lib = root.join(format!(
        "target/newlib-{arch}/{arch}-unknown-myos/lib"
    ));
    println!("cargo:rerun-if-changed={}", lib.join("libmbedtls.a").display());

    let glue = manifest.join("src/platform.c");
    println!("cargo:rerun-if-changed={}", glue.display());
    let out_dir = env::var("OUT_DIR").unwrap();
    let obj = format!("{out_dir}/platform.o");
    let cc = format!("{arch}-unknown-myos-cc");
    let mut cmd = Command::new(&cc);
    cmd.env("PATH", &path);
    if Command::new("bash")
        .args(["-lc", &format!("command -v {cc}")])
        .env("PATH", &path)
        .status()
        .map(|s| !s.success())
        .unwrap_or(true)
    {
        cmd = Command::new("clang");
        cmd.arg(format!("--target={arch}-unknown-none"));
    }
    let clang_res = Command::new("clang")
        .arg("-print-resource-dir")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| format!("{}/include", s.trim()))
        .unwrap_or_default();
    if !clang_res.is_empty() {
        cmd.arg("-isystem").arg(&clang_res);
    }
    let status = cmd
        .arg("-ffreestanding")
        .arg("-fPIC")
        .arg("-Os")
        .arg("-nostdinc")
        .arg("-I")
        .arg(root.join("ports/mbedtls/include"))
        .arg("-isystem")
        .arg(&newlib_inc)
        .arg("-I")
        .arg(&inc)
        .arg("-I")
        .arg(root.join("ports/mbedtls"))
        .arg("-DMBEDTLS_CONFIG_FILE=\"myos_mbedtls_config.h\"")
        .arg("-c")
        .arg(&glue)
        .arg("-o")
        .arg(&obj)
        .env("PATH", &path)
        .status()
        .expect("compile platform.c");
    if !status.success() {
        panic!("platform.c compile failed");
    }
    // Archive into a static lib so the .o is linked into dependents of this
    // rlib (cargo:rustc-link-arg on a bare .o does not propagate from libs).
    let archive = format!("{out_dir}/libmyos_tls_plat.a");
    let ar_status = Command::new("ar")
        .args(["rcs", &archive, &obj])
        .status()
        .expect("ar platform.o");
    if !ar_status.success() {
        panic!("ar libmyos_tls_plat.a failed");
    }
    // Link order: dependents before providers (plat → mbedtls → newlib).
    println!("cargo:rustc-link-search=native={out_dir}");
    println!("cargo:rustc-link-lib=static=myos_tls_plat");
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=mbedtls");
    println!("cargo:rustc-link-lib=static=mbedx509");
    println!("cargo:rustc-link-lib=static=mbedcrypto");
    // mbedtls needs libc string/time/alloc; reuse newlib+libgloss (no crt0).
    println!("cargo:rustc-link-search=native={}", newlib_lib.display());
    println!("cargo:rustc-link-lib=static=c");
    println!("cargo:rustc-link-lib=static=gloss");
    println!("cargo:rustc-link-lib=static=g");
}
