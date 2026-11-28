# awk, installed as an app: fields and arithmetic, an associative array
# walked in a for, a regular expression with a substitution, printf's
# formats, a program from a file with a variable set on the command line
# (-v) and another separator (-F), getline from a command and print to
# one (libgloss's popen and pclose).
awk_run() {
	out=$(printf 'a 1\nb 2\na 3\n' | run-myos awk '{ s[$1] += $2 } END { printf "%d %d %.2f\n", s["a"], s["b"], s["a"] / 3 }')
	echo "$out"
	[ "$out" = "4 2 1.33" ] || return 1
	out=$(printf 'x=1;y=22\n' | run-myos awk -F';' '{ n = gsub(/[0-9]+/, "<&>"); print n, $2, length($0), toupper(substr($1, 1, 1)) }')
	echo "$out"
	[ "$out" = "2 y=<22> 12 X" ] || return 1
	printf 'BEGIN { "echo sub" | getline w; split("p:q:r", a, ":") }\n{ print v, w, a[NR], NF }\n' > /tmp/awk-test.awk
	out=$(printf 'one\ntwo words\n' | run-myos awk -v v=set -f /tmp/awk-test.awk)
	echo "$out"
	[ "$out" = "set sub p 1
set sub q 2" ] || return 1
	out=$(run-myos awk 'BEGIN { print "b\na" | "sort"; r = close("sort"); print r }')
	echo "$out"
	[ "$out" = "a
b
0" ]
}
t awk awk_run
