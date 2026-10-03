# `ok` runs the slim VFS checks init ran at boot; "received: \x" is the
# shell reporting non-printable input (the real-hardware `ok` -> `/???` bug).
ok_checks() {
	capture $OUT/ok.log ok
	cat $OUT/ok.log
	! contains "received: \\x" $OUT/ok.log
}
t ok ok_checks
