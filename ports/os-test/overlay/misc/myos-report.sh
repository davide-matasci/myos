# Summarize every out/**.out: pass = ran and exited 0; compile_error = tcc
# could not build it; everything else failed at runtime.
# Enumerate with find -print (one path per line), not $(find | sort):
# oksh/make command-substitution over a pipe can hang on myos when the
# writer side's EOF never wakes the reader (same class as Makefile
# $(shell find …) which we already banned). Sort is unnecessary for the
# counters; failure listing order is best-effort.
# Do not use pipelines inside $(…) here. Avoid `tail` when printing
# failures — under UEFI RAM pressure an extra fork mid-report OOMs the
# frame allocator before `$` returns.
total=0; pass=0; cerr=0; fail=0
if [ -d out ]; then
  find out -name '*.out' -print > /tmp/os-test-outs.list 2>/dev/null || true
else
  : > /tmp/os-test-outs.list
fi
while IFS= read -r f || [ -n "$f" ]; do
  [ -z "$f" ] && continue
  total=$((total+1))
  case "$(cat "$f" 2>/dev/null)" in
    compile_error) cerr=$((cerr+1)) ;;
    *"exit: "*) fail=$((fail+1)) ;;
    *) pass=$((pass+1)) ;;
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
while IFS= read -r f || [ -n "$f" ]; do
  [ -z "$f" ] && continue
  body=$(cat "$f" 2>/dev/null)
  case "$body" in
    compile_error) echo "CE  ${f#out/}" ;;
    *"exit: "*)
      # Print the exit marker without forking `tail` (UEFI OOM mid-report).
      echo "F   ${f#out/} exit:${body##*exit:}"
      ;;
  esac
done < /tmp/os-test-outs.list
