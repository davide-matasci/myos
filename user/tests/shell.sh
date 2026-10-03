# The shell, exec and the basic programs: what every boot must do.

sh_nosuch() {
	nosuchcmd 2>&1 | grep -q "not found"
}
# `ok` runs the slim VFS checks init ran at boot; "received: \x" is the
# shell reporting non-printable input (the real-hardware `ok` -> `/???` bug).
sh_ok() {
	capture $OUT/ok.log ok
	cat $OUT/ok.log
	! contains "received: \\x" $OUT/ok.log
}
sh_echo() {
	[ "$(echo test)" = test ]
}
sh_pipe() {
	[ "$(echo pipe | cat)" = pipe ]
}
sh_sbase_echo() {
	[ "$(/bin/sbase/echo hi)" = hi ]
}
# oksh's redirect uses newlib O_CREAT; it must create on the tmpfs.
sh_redirect() {
	echo test > /tmp/aaa && [ "$(cat /tmp/aaa)" = test ]
}
# `which` walks $PATH with fstatat: an absolute hit, never "not an external command".
sh_which() {
	w=$(which ls)
	echo "$w"
	case $w in
	/*/ls) return 0 ;;
	esac
	return 1
}
# exec limits: 40 arguments and a 711-byte environment string through oksh
# (libgloss execve) into sbase programs.
sh_exec_limits() {
	A="a b c d e f g h"
	A="$A $A $A $A $A"
	X=$A$A$A$A$A$A$A$A$A
	set -- $(/bin/sbase/echo $A)
	Y=$(X=$X /bin/sbase/printenv X)
	echo "args=$# env=${#Y}"
	[ $# -eq 40 ] && [ ${#Y} -eq 711 ]
}

t shell_nosuch sh_nosuch
t shell_ok sh_ok
t shell_echo sh_echo
t shell_pipe sh_pipe
t uutils_true /bin/coreutils/true
t sbase_echo sh_sbase_echo
t shell_redirect sh_redirect
t shell_which sh_which
t exec_limits sh_exec_limits

# The heavy smoke (user/heap): std and C programs, sbase, uutils, find,
# ripgrep, tcc, the socket and ping clients, FPU state across preemption,
# threads; git's porcelain in the full mode (a package). It prints a
# `[ OK ] <stage>` line per stage and `[ OK ] smoke` at the end.
heap_smoke() {
	if [ "$MODE" = full ]; then
		capture $OUT/heap.log heap
	else
		capture $OUT/heap.log heap mini
	fi
	rc=$?
	cat $OUT/heap.log
	[ $rc -eq 0 ] || { echo "heap exit $rc"; return 1; }
	for n in std "std cat" "std echo" bigalloc c sbase sls "uutils echo" "uutils true" \
		"uutils false" find "uutils cat" "uutils ls" ripgrep "sbase argv" ping socket fpu threads smoke; do
		contains "[ OK ] $n" $OUT/heap.log || { echo "missing: [ OK ] $n"; return 1; }
	done
	contains /tmp/findnest/a/b/c $OUT/heap.log || { echo "missing: find output"; return 1; }
	if [ -x /bin/tcc/tcc ]; then
		for n in tcc "tcc std"; do
			contains "[ OK ] $n" $OUT/heap.log || { echo "missing: [ OK ] $n"; return 1; }
		done
	fi
	if [ "$MODE" = full ] && [ -x /bin/custom/git ]; then
		for n in git "git commit"; do
			contains "[ OK ] $n" $OUT/heap.log || { echo "missing: [ OK ] $n"; return 1; }
		done
	fi
	# heap's own failure lines ("threads FAIL", "fpu FAIL: ..."); a path
	# such as EAI_FAIL.c in a listing is not one.
	! grep -q -E "(^| )FAIL" $OUT/heap.log
}
t heap heap_smoke
