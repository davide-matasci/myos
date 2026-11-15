# rustc, installed as an app on the scratch disk (`disk`: the compiler's
# library is larger than a tmpfs file may be, 128 MiB, and the apps of the
# other packages' tests are in /tmp/apps): the compiler and cargo run on
# myos and build myos programs. The disk is formatted here unless a test
# before left it mounted at /tmp/disk. A boot of their own in CI
# (scripts/ci-select.py, the `rustc` group): minutes of rustc under
# emulation, and the scratch disk to themselves.
rustc_root() {
	d=/tmp/disk
	if ! grep -q " $d ext2 " /proc/mounts; then
		mkdir -p $d && mkfs.ext2 /dev/nvme1n1/data > /dev/null && mount /dev/nvme1n1/data $d ext2 || return 1
	fi
	[ -e $d/apps/rustc/app ] && return 0
	# get-myos's lines go to the console (fd 3): the host ends a boot that
	# is silent for 600 s.
	get-myos -r $d/apps -m $MIRROR rustc 2>&3
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
	run-myos -r /tmp/disk/apps rustc -O -o /tmp/rustc-hello /tmp/rustc-hello.rs || return 1
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
	(cd $p && CARGO_HOME=/tmp/disk/cargo-home run-myos -r /tmp/disk/apps rustc:cargo build --release --offline) >&3 2>&1 || return 1
	echo "rustc_cargo: built in $(($(date +%s) - t0)) s" >&3
	out=$($p/target/release/hello) || return 1
	echo "$out"
	case $out in
	"hello from cargo: 42 "*-unknown-myos-mlibc) ;;
	*) return 1 ;;
	esac
}
t -t net,heavy,disk,exec,dl rustc_cargo rustc_cargo
