# The heavy smoke: std and C programs, sbase, find, ripgrep, tcc, the socket
# and ping clients, FPU state across preemption, threads; uutils and git's
# porcelain in the full mode (packages, installed before the tests). It
# prints a `[ OK ] <stage>` line per stage and `[ OK ] smoke` at the end.
heap_smoke() {
	if [ "$MODE" = full ]; then
		capture $OUT/heap.log heap
	else
		capture $OUT/heap.log heap mini
	fi
	rc=$?
	cat $OUT/heap.log
	[ $rc -eq 0 ] || { echo "heap exit $rc"; return 1; }
	for n in std "std cat" "std echo" bigalloc c sbase sls find ripgrep "sbase argv" ping socket fpu \
		threads smoke; do
		contains "[ OK ] $n" $OUT/heap.log || { echo "missing: [ OK ] $n"; return 1; }
	done
	if [ "$MODE" = full ]; then
		for n in "uutils echo" "uutils true" "uutils false" "uutils cat" "uutils ls"; do
			contains "[ OK ] $n" $OUT/heap.log || { echo "missing: [ OK ] $n"; return 1; }
		done
	fi
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
