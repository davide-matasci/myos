# Compile and run one os-test test on myos: tcc + newlib sysroot.
# Usage: myos-run.sh <cc> <cflags> <src.c> <test-name>
# Outcome semantics mirror upstream misc/run.sh: on compile failure the
# outcome word is the whole .out; on run, a nonzero exit appends
# "exit: CODE" (any nonzero is a failure here — tests use errx(1, ...)).
set -u
CC="$1"; CFLAGS="$2"; SRC="$3"; T="$4"
OUT="out/$T.out"
BIN="$T"
mkdir -p "${OUT%/*}" "${BIN%/*}" || echo "myos-run: mkdir failed rc=$? dir=${OUT%/*}"
rm -f -- "$OUT" "$BIN"
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
