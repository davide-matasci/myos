# Grade every out/**.out the way upstream's html.c does: a test whose suite
# ships expectations (SUITE.expect/NAME.*, from the suite here or
# /lib/os-test) passes when its outcome equals one of them; a test without
# any passes on "exit: 0" (myos-run.sh writes the outcome as upstream's
# run.sh does). NAME.unknown.* are outcomes upstream has seen but not judged
# good or bad: matching one passes here (the suite claims nothing against
# it), where html.c reports "unknown".
# compile_error is counted apart. So a test whose POSIX outcome is an error
# (open: ENOTDIR) passes by printing it, not by exiting 0.
# Enumerate with find -print (one path per line), not $(find | sort):
# oksh/make command-substitution over a pipe can hang on myos when the
# writer side's EOF never wakes the reader (same class as Makefile
# $(shell find …) which we already banned). Sort is unnecessary for the
# counters; failure listing order is best-effort.
#
# CRITICAL: do not fork `cat`/`tail` per file. make's aspace is large after
# 137 tests; each fork+exec copies it (mm exec site) and mid-report OOM'd
# bios/uefi after pass_rate= was printed (runs 34993401424 / 34997440611).
# Read files with shell redirection only.
total=0; pass=0; cerr=0; fail=0
: > /tmp/os-test-fails.list
if [ -d out ]; then
  find out -name '*.out' -print > /tmp/os-test-outs.list 2>/dev/null || true
else
  : > /tmp/os-test-outs.list
fi

# The file $1's lines joined by newlines (no trailing one), in $text.
slurp() {
  text=
  _first=1
  while IFS= read -r _line || [ -n "$_line" ]; do
    if [ "$_first" = 1 ]; then
      text=$_line
      _first=
    else
      text="$text
$_line"
    fi
  done < "$1"
}

while IFS= read -r f || [ -n "$f" ]; do
  [ -z "$f" ] && continue
  total=$((total+1))
  outcome=
  [ -f "$f" ] && slurp "$f" && outcome=$text
  if [ "$outcome" = compile_error ]; then
    cerr=$((cerr+1))
    echo "CE  ${f#out/}" >> /tmp/os-test-fails.list
    continue
  fi
  # out/SUITE/NAME.out against SUITE.expect/NAME.*
  t=${f#out/}
  t=${t%.out}
  suite=${t%%/*}
  name=${t#*/}
  exp=$suite.expect
  [ -d "$exp" ] || exp=/lib/os-test/$suite.expect
  rated=
  good=
  for e in "$exp/$name".*; do
    [ -f "$e" ] || continue
    rated=1
    slurp "$e"
    if [ "$text" = "$outcome" ]; then
      good=1
      break
    fi
  done
  if [ -z "$rated" ] && [ "$outcome" = "exit: 0" ]; then
    good=1
  fi
  if [ -n "$good" ]; then
    pass=$((pass+1))
  else
    fail=$((fail+1))
    first=
    [ -f "$f" ] && IFS= read -r first < "$f"
    [ -n "$first" ] || first="no outcome"
    echo "F   ${f#out/}: $first" >> /tmp/os-test-fails.list
  fi
done < /tmp/os-test-outs.list
if [ "$total" -gt 0 ]; then
  pass_rate=$((pass * 100 / total))
else
  pass_rate=0
fi
echo "=== os-test: $total tests, $pass pass, $fail fail, $cerr compile_error ==="
echo "pass_rate=${pass_rate}% ($pass/$total)"
echo "--- failures and compile errors ---"
if [ "$fail" -eq 0 ] && [ "$cerr" -eq 0 ]; then
  echo "(none)"
else
  while IFS= read -r line || [ -n "$line" ]; do
    [ -n "$line" ] && echo "$line"
  done < /tmp/os-test-fails.list
fi
