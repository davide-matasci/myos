# Compile and run one os-test test on myos: tcc + newlib sysroot.
# Usage: myos-run.sh <cc> <cflags> <src.c> <test-name>
# Outcome semantics mirror upstream misc/run.sh: on compile failure the
# outcome word is the whole .out; on run, a nonzero exit appends
# "exit: CODE" (any nonzero is a failure here — tests use errx(1, ...)).
#
# Boot CI thin smoke: if prebuilt/$T exists (host-built ELF packed at
# /lib/os-test/prebuilt/… and staged by ci-smoke-copy.sh), skip guest tcc
# and only exec. Guest tcc under x86 TCG is ~60–100× slower than heap's
# tiny compile; 22 smokes cannot fit the 600s QEMU budget otherwise.
#
# Working directory: T is suite/rel (e.g. basic/fcntl/open). Upstream runs
# `make -C basic`, so tests that open("fcntl/open") / fopen("stdio/fopen") /
# stat("sys_stat/stat") resolve relative to the suite. We cd into the suite
# before exec to match that layout.
set -u
CC="$1"; CFLAGS="$2"; SRC="$3"; T="$4"
OUT="out/$T.out"
BIN="$T"
SUITE="${T%%/*}"
REL="${T#*/}"
# Prebuilt ELFs live read-only in the initramfs; exec in place (no tmpfs copy
# — hundreds of cp/chmod forks stalled the TCG boot window). Writable cwd
# copies remain supported for manual/debug runs.
PREBUILT=""
PREBUILT_RO=0
if [ -x "prebuilt/$T" ]; then
	PREBUILT="prebuilt/$T"
# libfs registers initramfs files mode 0444, so /lib/os-test/prebuilt ELFs
# are readable but not executable: test readability, copy to the writable
# BIN path and chmod there (exec-in-place fails with EACCES → sh 126).
elif [ -r "/lib/os-test/prebuilt/$T" ]; then
	PREBUILT="/lib/os-test/prebuilt/$T"
	PREBUILT_RO=1
fi
mkdir -p "${OUT%/*}" "${BIN%/*}" || echo "myos-run: mkdir failed rc=$? dir=${OUT%/*}"
rm -f -- "$OUT" "$BIN"

run_in_suite() {
	# Run from the suite directory so relative open/fopen/stat paths match
	# upstream. OUT stays repo-root-relative; use an absolute-ish path via
	# prefix when cwd changes.
	if [ "$SUITE" = "$T" ] || [ -z "$REL" ] || [ "$REL" = "$T" ]; then
		"$1" > "$OUT" 2>&1
	else
		(cd "$SUITE" && ./$REL) > "$OUT" 2>&1
	fi
	return $?
}

if [ -n "$PREBUILT" ]; then
	# Progress for serial CI (unbuffered line so long suites are not silent).
	echo "os-test: $T (prebuilt)"
	# Copy to the writable tcc target path so tests that open("$REL") or
	# sibling paths behave like the compile+run flow.
	cp "$PREBUILT" "$BIN" || exit 0
	chmod +x "$BIN" || true
	run_in_suite "$BIN"
	CODE=$?
	# Capture $? before any if-statement: oksh resets $? to the if
	# statement's own status (0 when the condition fails and there is no
	# else), which reported every failed test as "exit: 0".
	if [ "$CODE" -ne 0 ]; then
		echo "exit: $CODE" >> "$OUT"
	fi
	exit 0
fi

echo "os-test: $T"
if ! "$CC" $CFLAGS "$SRC" -o "$BIN" -lm 2> "out/$T.err"; then
	echo compile_error > "$OUT"
	exit 0
fi
run_in_suite "$BIN"
CODE=$?
if [ "$CODE" -eq 0 ]; then
	exit 0
fi
echo "exit: $CODE" >> "$OUT"
exit 0
