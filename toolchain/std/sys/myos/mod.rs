pub mod abi;
/// The C library under `std` on the mlibc flavour of the target
/// (`target_env = "mlibc"`, `docs/userspace.md` "Rust userspace").
#[cfg(target_env = "mlibc")]
pub mod libc;
