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
# Prefer host-prebuilt ELFs. libfs registers /lib files mode 0755, so we can
# exec /lib/os-test/prebuilt/$T in place — no tmpfs cp/chmod per test (that
# path used to burn ~18 MiB × N of anonymous pages across the curated suite
# and helped trip mm.rs OOM under 3072 MiB QEMU).
PREBUILT=""
if [ -x "prebuilt/$T" ]; then
	PREBUILT="prebuilt/$T"
elif [ -r "/lib/os-test/prebuilt/$T" ]; then
	PREBUILT="/lib/os-test/prebuilt/$T"
fi
mkdir -p "${OUT%/*}" || echo "myos-run: mkdir failed rc=$? dir=${OUT%/*}"
# Only need BIN dir when compiling or when falling back to a cwd copy.
if [ -z "$PREBUILT" ]; then
	mkdir -p "${BIN%/*}" || echo "myos-run: mkdir failed rc=$? dir=${BIN%/*}"
fi
rm -f -- "$OUT" "$BIN"

run_in_suite() {
	# $1 = path to ELF. Absolute paths survive `cd "$SUITE"`; relative
	# suite/rel paths are invoked as ./$REL after cd (compile flow).
	if [ "$SUITE" = "$T" ] || [ -z "$REL" ] || [ "$REL" = "$T" ]; then
		"$1" > "$OUT" 2>&1
	else
		case "$1" in
		/*)
			(cd "$SUITE" && "$1") > "$OUT" 2>&1
			;;
		prebuilt/*)
			# Resolve before cd so suite-relative cwd still finds it.
			abs="$(pwd)/$1"
			(cd "$SUITE" && "$abs") > "$OUT" 2>&1
			;;
		*)
			(cd "$SUITE" && ./$REL) > "$OUT" 2>&1
			;;
		esac
	fi
	return $?
}

if [ -n "$PREBUILT" ]; then
	# Progress for serial CI (unbuffered line so long suites are not silent).
	echo "os-test: $T (prebuilt)"
	run_in_suite "$PREBUILT"
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
if [ "$CODE" -ne 0 ]; then
	echo "exit: $CODE" >> "$OUT"
fi
rm -f -- "$BIN"
exit 0
