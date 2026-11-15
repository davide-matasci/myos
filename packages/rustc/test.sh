# rustc, installed as an app on the scratch disk (`disk`, the runner's
# disk_apps: the compiler's library alone is larger than a tmpfs file may
# be): the compiler and cargo run on myos and build myos programs. In
# CI's `disk` boot (scripts/ci-select.py), with the llvm package's tests.
rustc_root() {
	disk_apps rustc
}
# A program with a generic, a closure, a formatted string and a thread,
# compiled (LLVM in the compiler, lld from the llvm app), linked against
# mlibc and run.
rustc_hello() {
	rustc_root || return 1
	cat > /tmp/rustc-hello.rs <<'EOT'
use std::collections::BTreeMap;
fn main() {
    let words = ["rust", "on", "myos"];
    let lens: BTreeMap<&str, usize> = words.iter().map(|w| (*w, w.len())).collect();
    let total = std::thread::spawn(move || lens.values().sum::<usize>()).join().unwrap();
    println!("{} {}", words.join(" "), total);
}
EOT
	t0=$(date +%s)
	run-myos -r $DISK_APPS rustc -O -o /tmp/rustc-hello /tmp/rustc-hello.rs || return 1
	echo "rustc_hello: compiled in $(($(date +%s) - t0)) s" >&3
	out=$(/tmp/rustc-hello)
	echo "$out"
	[ "$out" = "rust on myos 10" ]
}
t -t net,heavy,disk,exec rustc_hello rustc_hello
# cargo builds a package with a build script (a program of the host's,
# run by cargo) and a proc macro of its own (a shared object rustc loads),
# offline, on the disk; myos runs the result.
rustc_cargo() {
	rustc_root || return 1
	p=/tmp/disk/hello
	rm -rf $p
	mkdir -p $p/src $p/pm/src || return 1
	printf '[package]\nname = "hello"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\npm = { path = "pm" }\n' > $p/Cargo.toml
	printf '[package]\nname = "pm"\nversion = "0.1.0"\nedition = "2021"\n\n[lib]\nproc-macro = true\n' > $p/pm/Cargo.toml
	echo 'use proc_macro::TokenStream; #[proc_macro] pub fn answer(_: TokenStream) -> TokenStream { "6 * 7".parse().unwrap() }' > $p/pm/src/lib.rs
	echo 'fn main() { println!("cargo:rustc-env=BUILT_ON={}", std::env::var("HOST").unwrap()); }' > $p/build.rs
	echo 'fn main() { println!("hello from cargo: {} {}", pm::answer!(), env!("BUILT_ON")); }' > $p/src/main.rs
	t0=$(date +%s)
	(cd $p && CARGO_HOME=/tmp/disk/cargo-home run-myos -r $DISK_APPS rustc:cargo build --release --offline) >&3 2>&1 || return 1
	echo "rustc_cargo: built in $(($(date +%s) - t0)) s" >&3
	out=$($p/target/release/hello) || return 1
	echo "$out"
	case $out in
	"hello from cargo: 42 "*-unknown-myos-mlibc) ;;
	*) return 1 ;;
	esac
}
t -t net,heavy,disk,exec,dl rustc_cargo rustc_cargo
