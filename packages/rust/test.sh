# rust, installed as an app (with the network and the Linux layer): the
# toolchain set up in a root on the scratch disk (formatted here unless
# the ext2 tests left it mounted at /tmp/disk), then cargo builds a myos
# program there with a build script and a proc macro of its own (the
# host's: linked by Alpine's lld against the root's musl and dlopened by
# rustc) and myos runs it natively. The first setup downloads the
# toolchain (some 230 MB) and takes minutes under emulation.
rust_cargo() {
	d=/tmp/disk
	if grep -q " $d ext2 " /proc/mounts; then
		rust_build $d/rust
		return
	fi
	mkdir -p $d && mkfs.ext2 /dev/nvme1n1/data > /dev/null && mount /dev/nvme1n1/data $d ext2 || return 1
	rust_build $d/rust
	rc=$?
	# The Linux layer bound /dev, /proc and /net into the root, for good.
	for b in dev proc net; do
		umount $d/rust/$b 2> /dev/null
	done
	umount $d || grep " $d" /proc/mounts
	return $rc
}
rust_build() {
	r=$1
	# setup's lines go to the console (fd 3): the host ends a boot that is
	# silent for 600 s.
	run-myos rust -r $r setup >&3 2>&1 || return 1
	p=$r/src/hello
	mkdir -p $p/src $p/pm/src || return 1
	printf '[package]\nname = "hello"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\npm = { path = "pm" }\n' > $p/Cargo.toml
	printf '[package]\nname = "pm"\nversion = "0.1.0"\nedition = "2021"\n\n[lib]\nproc-macro = true\n' > $p/pm/Cargo.toml
	echo 'use proc_macro::TokenStream; #[proc_macro] pub fn answer(_: TokenStream) -> TokenStream { "6 * 7".parse().unwrap() }' > $p/pm/src/lib.rs
	echo 'fn main() { println!("cargo:rustc-env=BUILT_ON={}", std::env::var("HOST").unwrap()); }' > $p/build.rs
	echo 'fn main() { println!("hello from myos: {} {}", pm::answer!(), env!("BUILT_ON")); }' > $p/src/main.rs
	(cd $p && run-myos rust -r $r cargo build --release) >&3 2>&1 || return 1
	out=$($p/target/*-unknown-myos/release/hello) || return 1
	echo "$out"
	case $out in
	"hello from myos: 42 "*-linux-musl) ;;
	*) return 1 ;;
	esac
	# Outside the root, the command says where to run it.
	! (cd / && run-myos rust -r $r cargo --version) 2> /tmp/rust-outside.err \
		&& grep -q 'inside' /tmp/rust-outside.err
}
t -t net,heavy,linux rust_cargo rust_cargo
