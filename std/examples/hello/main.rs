// Placeholder for `cargo +nightly build -Z build-std=std,panic_abort
// --target ../../targets/x86_64-unknown-myos.json` after patching Rust.
//
// fn main() {
//     println!("std ok");
// }

fn main() {
    eprintln!("Build with a patched sysroot; see std/pal/README.md");
}
