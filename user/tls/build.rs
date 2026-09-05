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
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=mbedtls");
    println!("cargo:rustc-link-lib=static=mbedx509");
    println!("cargo:rustc-link-lib=static=mbedcrypto");
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
    let status = cmd
        .arg("-ffreestanding")
        .arg("-fPIC")
        .arg("-Os")
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
    println!("cargo:rustc-link-arg={obj}");
}
