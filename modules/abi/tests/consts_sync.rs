//! The Rust `std` port (`toolchain/std/sys/myos/abi.rs`) cannot depend on
//! this crate, so it copies the few ABI values it needs. This test keeps
//! the copies honest: a new failure sentinel or a changed limit here must
//! reach them too (`myos_user` takes them from the crate itself).

use std::fs;
use std::path::Path;

/// The integer in `std`'s `pub const NAME: TYPE = (usize::MAX - N) as isize;`.
fn sentinel_offset(src: &str, name: &str) -> usize {
    let line = src.lines().find(|l| l.contains(&format!("pub const {name}:"))).unwrap_or_else(|| panic!("{name} not in std's abi.rs"));
    let n = line.split("usize::MAX - ").nth(1).and_then(|r| r.split(')').next()).unwrap_or_else(|| panic!("{name}: not `usize::MAX - N`: {line}"));
    n.trim().parse().unwrap()
}

/// The integer in `std`'s `pub const NAME: usize = N;`.
fn plain(src: &str, name: &str) -> usize {
    let line = src.lines().find(|l| l.contains(&format!("pub const {name}:"))).unwrap_or_else(|| panic!("{name} not in std's abi.rs"));
    line.split('=').nth(1).unwrap().trim().trim_end_matches(';').parse().unwrap()
}

#[test]
fn std_port_copies_agree() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../toolchain/std/sys/myos/abi.rs");
    let src = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(usize::MAX - sentinel_offset(&src, "SYSERR_EEXIST"), myos_abi::MYOS_SYSERR_EEXIST);
    assert_eq!(usize::MAX - sentinel_offset(&src, "SYSERR_ENAMETOOLONG"), myos_abi::MYOS_SYSERR_ENAMETOOLONG);
    assert_eq!(plain(&src, "PATH_MAX"), myos_abi::MYOS_PATH_MAX);
}
