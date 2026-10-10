# lua, installed as an app: its own version check (a double compare,
# which the riscv64 soft float got wrong once), and double arithmetic and
# formatting through the interpreter.
lua_run() {
	run-myos lua -v | grep '^Lua 5' || return 1
	out=$(run-myos lua -e 'print(80.0 + 100, 80.5 * 1000, 1 / 8, 2.5 < 3, string.format("%.2f", 1e3 / 7))')
	echo "$out"
	[ "$out" = "180.0	80500.0	0.125	true	142.86" ]
}
t lua lua_run
