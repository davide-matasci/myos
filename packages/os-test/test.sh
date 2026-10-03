# The curated os-test list (misc/ci-boot.tests: POSIX core, non-basic,
# signal handlers, myos chroot/FIFO; 319 prebuilt tests): a thin writable
# copy of the suite under /tmp/o, `make report`, and every test must pass.
# Full mode only (the list takes a while under TCG). The report's progress
# goes to the console: one line per test.
[ "$MODE" = full ] || return 0

ostest_report() {
	sh /lib/os-test/misc/ci-smoke-copy.sh /tmp/o || return 1
	(cd /tmp/o && make TESTLIST=misc/ci-boot.tests report 2>&1 | tee /tmp/o/report.log >&3)
	grep -e "^pass_rate=" -e "^F " -e "^C " -e "=== os-test" /tmp/o/report.log
	rate=$(grep "^pass_rate=" /tmp/o/report.log | head -n 1)
	[ -n "$rate" ] || { echo "no pass_rate line"; return 1; }
	pt=${rate#*(}
	pt=${pt%)*}
	[ "${pt%/*}" = "${pt#*/}" ] && [ "${pt#*/}" -gt 0 ]
}
# setpwent (basic/pwd): a non-empty .out is a compile error or a bad exit.
ostest_setpwent() {
	[ -f /tmp/o/out/basic/pwd/setpwent.err ] && [ ! -s /tmp/o/out/basic/pwd/setpwent.out ]
}
t os_test ostest_report
t os_test_setpwent ostest_setpwent
