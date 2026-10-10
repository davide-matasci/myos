# uutils coreutils, installed as an app: the multicall ELF runs under one
# of its names (run-myos coreutils:NAME).
t uutils_true run-myos coreutils:true

# File times through Rust std and rustix: uutils touch creates the file
# (std's writable open) and sets them (utimensat in the libc crate), ls -l
# reads them (fs::Metadata).
uutils_touch() {
	rm -f /tmp/ut
	run-myos coreutils:touch -t 200001020304 /tmp/ut || return 1
	l=$(run-myos coreutils:ls -l /tmp/ut)
	echo "ls: $l"
	rm /tmp/ut
	case $l in *"Jan  2  2000"*) ;; *) return 1 ;; esac
}
t uutils_touch uutils_touch

# Moving a directory (issue #314): uutils mv canonicalizes the source and
# target to reject moving a directory into itself; that path must not return
# std's "operation not supported" (canonicalize was an unsupported stub).
uutils_mv_dir() {
	rm -rf /tmp/mvd /tmp/mvd2
	mkdir -p /tmp/mvd/sub || return 1
	echo hi >/tmp/mvd/sub/f || return 1
	out=$(run-myos coreutils:mv /tmp/mvd /tmp/mvd2 2>&1) || { echo "mv: $out"; return 1; }
	[ -f /tmp/mvd2/sub/f ] && [ ! -e /tmp/mvd ] || return 1
	rm -rf /tmp/mvd2
}
t uutils_mv_dir uutils_mv_dir
