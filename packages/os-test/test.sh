# The curated os-test list (misc/ci-boot.tests: POSIX core, non-basic,
# signal handlers, myos chroot/FIFO, the udp suite and the rest of the
# suite that passes; 1040 prebuilt tests): a thin writable copy of the
# suite under /tmp/o,
# `make report`, and every test must pass.
# Full mode only (the list takes a while under TCG). The report's progress
# goes to the console: one line per test.
[ "$MODE" = full ] || return 0

ostest_report() {
	sh /lib/os-test/misc/ci-smoke-copy.sh /tmp/o || return 1
	(cd /tmp/o && make TESTLIST=misc/ci-boot.tests report 2>&1 | tee /tmp/o/report.log >&3)
	grep -e "^pass_rate=" -e "^F " -e "^C " -e "=== os-test" /tmp/o/report.log
	rate=$(grep "^pass_rate=" /tmp/o/report.log | head -n 1)
	[ -n "$rate" ] || { echo "no pass_rate line"; return 1; }
	# "pass_rate=NN% (P/T)": every test passed when P = T (sed, not ${rate#*(}:
	# oksh reads the parenthesis in a pattern as an unclosed group).
	pt=$(echo "$rate" | sed 's/.*(\(.*\)).*/\1/')
	[ "${pt%/*}" = "${pt#*/}" ] && [ "${pt#*/}" -gt 0 ]
}
# setpwent (basic/pwd): the runner writes the test's outcome, "exit: 0" when
# it ran clean (anything else is a compile error, output or a bad exit).
ostest_setpwent() {
	[ -f /tmp/o/out/basic/pwd/setpwent.out ] || return 1
	IFS= read -r outcome < /tmp/o/out/basic/pwd/setpwent.out
	[ "$outcome" = "exit: 0" ]
}
t os_test ostest_report
t os_test_setpwent ostest_setpwent
