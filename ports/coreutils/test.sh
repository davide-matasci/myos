# uutils coreutils: the multicall ELF runs under one of its names.
t uutils_true /bin/coreutils/true

# File times through Rust std and rustix: uutils touch creates the file
# (std's writable open) and sets them (utimensat in the libc crate), ls -l
# reads them (fs::Metadata).
uutils_touch() {
	rm -f /tmp/ut
	/bin/coreutils/touch -t 200001020304 /tmp/ut || return 1
	l=$(/bin/coreutils/ls -l /tmp/ut)
	echo "ls: $l"
	rm /tmp/ut
	case $l in *"Jan  2  2000"*) ;; *) return 1 ;; esac
}
t uutils_touch uutils_touch
