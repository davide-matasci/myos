# Summarize out/*/*.out: pass = ran and exited 0; compile_error = tcc
# could not build it; everything else failed at runtime.
total=0; pass=0; cerr=0; fail=0
for f in out/*/*.out; do
  total=$((total+1))
  case "$(cat "$f")" in
    compile_error) cerr=$((cerr+1)) ;;
    *"exit: "*) fail=$((fail+1)) ;;
    *) pass=$((pass+1)) ;;
  esac
done
echo "=== os-test: $total tests, $pass pass, $fail fail, $cerr compile_error ==="
echo "--- failures and compile errors ---"
for f in out/*/*.out; do
  case "$(cat "$f")" in
    compile_error) echo "CE  ${f#out/}" ;;
    *"exit: "*) echo "F   ${f#out/} $(tail -1 "$f")" ;;
  esac
done
