# Summarize every out/**.out: pass = ran and exited 0; compile_error = tcc
# could not build it; everything else failed at runtime.
# Enumerate with find, not a glob: suites are nested (out/basic/pwd/*.out)
# and the guest shell does not glob across levels.
total=0; pass=0; cerr=0; fail=0
for f in $(find out -name '*.out' | sort); do
  total=$((total+1))
  case "$(cat "$f")" in
    compile_error) cerr=$((cerr+1)) ;;
    *"exit: "*) fail=$((fail+1)) ;;
    *) pass=$((pass+1)) ;;
  esac
done
echo "=== os-test: $total tests, $pass pass, $fail fail, $cerr compile_error ==="
echo "--- failures and compile errors ---"
for f in $(find out -name '*.out' | sort); do
  case "$(cat "$f")" in
    compile_error) echo "CE  ${f#out/}" ;;
    *"exit: "*) echo "F   ${f#out/} $(tail -1 "$f")" ;;
  esac
done
