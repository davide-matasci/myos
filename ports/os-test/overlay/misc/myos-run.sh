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
set -u
CC="$1"; CFLAGS="$2"; SRC="$3"; T="$4"
OUT="out/$T.out"
BIN="$T"
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

if [ -n "$PREBUILT" ]; then
	# Progress for serial CI (unbuffered line so long suites are not silent).
	echo "os-test: $T (prebuilt)"
	if [ "$PREBUILT_RO" = 1 ]; then
		# Read-only initramfs source: copy to the writable tcc target path
		# (tests that open("$T") or sibling paths behave like compile+run).
		cp "$PREBUILT" "$BIN" || exit 0
		chmod +x "$BIN" || true
		RUN="$BIN"
	else
		# Writable copy at the tcc target path so tests that open("$T") or
		# sibling paths behave like the compile+run flow.
		cp "$PREBUILT" "$BIN" || exit 0
		chmod +x "$BIN" || true
		RUN="$BIN"
	fi
	"$RUN" > "$OUT" 2>&1
	CODE=$?
	# Capture $ ? before any if-statement: oksh resets $? to the if
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
if "./$BIN" > "$OUT" 2>&1; then
	exit 0
fi
CODE=$?
echo "exit: $CODE" >> "$OUT"
exit 0
