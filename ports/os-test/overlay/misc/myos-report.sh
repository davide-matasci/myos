# Summarize every out/**.out: pass = ran and exited 0; compile_error = tcc
# could not build it; everything else failed at runtime.
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
while IFS= read -r f || [ -n "$f" ]; do
  [ -z "$f" ] && continue
  total=$((total+1))
  body=
  if [ -f "$f" ]; then
    # One line is enough for compile_error / "exit: N" markers.
    IFS= read -r body < "$f" || true
    # If the exit marker is on a later line, scan the rest without forking.
    case "$body" in
      compile_error|*"exit: "*) ;;
      *)
        while IFS= read -r line || [ -n "$line" ]; do
          case "$line" in
            *"exit: "*) body=$line; break ;;
          esac
        done < "$f"
        ;;
    esac
  fi
  case "$body" in
    compile_error)
      cerr=$((cerr+1))
      echo "CE  ${f#out/}" >> /tmp/os-test-fails.list
      ;;
    *"exit: "*)
      fail=$((fail+1))
      echo "F   ${f#out/} exit:${body##*exit:}" >> /tmp/os-test-fails.list
      ;;
    *)
      pass=$((pass+1))
      ;;
  esac
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
