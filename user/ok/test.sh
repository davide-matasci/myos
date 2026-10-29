# `ok` runs the slim VFS checks init ran at boot; "received: \x" is the
# shell reporting non-printable input (the real-hardware `ok` -> `/???` bug).
ok_checks() {
	capture $OUT/ok.log ok
	cat $OUT/ok.log
	! contains "received: \\x" $OUT/ok.log
}
t -t smoke ok ok_checks

# `ok lines`: the status lines two processes print at once come out whole
# (a status line is one write, myos_user::status_ok; CI run 2344 missed
# the boot marker `[ OK ] rc started`, netd's line in the middle of it).
ok_lines() {
	ok lines > $OUT/ok-lines.log 2>&1 || { cat $OUT/ok-lines.log; return 1; }
}
t -t smoke,console ok_lines ok_lines
