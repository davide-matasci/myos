//! The program's entry. Its std is in librt.so, a shared object, whose
//! `_start` no program can start at: mlibc's crt0.o ($MYOS_CRT0, from
//! user/std-mlibc/build.sh) is linked into the program itself.
fn main() {
    println!("cargo:rerun-if-env-changed=MYOS_CRT0");
    let crt0 = std::env::var("MYOS_CRT0").expect("MYOS_CRT0: mlibc's crt0.o");
    println!("cargo:rustc-link-arg-bins={crt0}");
}
