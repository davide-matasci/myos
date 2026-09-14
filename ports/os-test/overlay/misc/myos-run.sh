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
PREBUILT="prebuilt/$T"
mkdir -p "${OUT%/*}" "${BIN%/*}" || echo "myos-run: mkdir failed rc=$? dir=${OUT%/*}"
rm -f -- "$OUT" "$BIN"

if [ -x "$PREBUILT" ]; then
	# Progress for serial CI (unbuffered line so long suites are not silent).
	echo "os-test: $T (prebuilt)"
	# Install at the same path tcc would write so tests that open("$T") or
	# sibling paths behave like the compile+run flow.
	cp "$PREBUILT" "$BIN" || exit 0
	chmod +x "$BIN" || true
	if "./$BIN" > "$OUT" 2>&1; then
		exit 0
	fi
	CODE=$?
	echo "exit: $CODE" >> "$OUT"
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
