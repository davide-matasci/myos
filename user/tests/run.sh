#!/bin/sh
# The boot tests (docs/testing.md). `myos test-mini` and `myos test-full`
# boot an image, log in and type
#
#   sh /lib/myos-tests/run.sh mini|full
#
# which anyone can do by hand at the prompt too. One line per test on the
# console, which the host watches on the serial port:
#
#   TEST <name> PASS
#   TEST <name> FAIL (exit N)     then the test's output, indented
#   HOST <request...>             a request to the host (connect to a port)
#   TESTS DONE <passed>/<total>   at the end
#
# A test is a command (mostly a shell function); its exit status decides,
# its output goes to /tmp/myos-tests/<name>.out and is shown when it fails.
# Every test belongs to the port of what it tests (ports/*.sh, from their
# PORT_TEST; the packer names them so the core image ports come first, the
# other image ports next, the packages last), except the kernel's own
# (kernel.sh), which run first. The full mode first installs every package
# of the mirror the host serves.

MODE=${1:-mini}
TESTS=/lib/myos-tests
OUT=/tmp/myos-tests
MIRROR=${MYOS_MIRROR:-http://10.0.2.2:8765}
passed=0
total=0
mkdir -p $OUT

# fd 3 stays the console whatever a test redirects: the HOST requests and a
# long test's progress go there.
exec 3>&1

# capture FILE COMMAND...: the command's output, both streams, in the file.
capture() {
	f=$1
	shift
	"$@" > "$f" 2>&1
}

# t NAME COMMAND...: run one test.
t() {
	name=$1
	shift
	total=$((total + 1))
	capture $OUT/$name.out "$@"
	rc=$?
	if [ $rc -eq 0 ]; then
		passed=$((passed + 1))
		echo "TEST $name PASS"
	else
		echo "TEST $name FAIL (exit $rc)"
		tail -n 40 $OUT/$name.out | sed 's/^/    /'
	fi
}

# contains NEEDLE FILE: the file has the string somewhere.
contains() {
	grep -q -F -- "$1" "$2"
}

# The full boot gets every package of this build from the host's mirror
# first (docs/packages.md; packages.txt names the ports the image lacks,
# the index has the image's ports too): the tests of the packages and
# `heap`'s git stage then find them at their image paths.
install_packages() {
	names=$(curl -fsS $MIRROR/packages.txt | tr "\n" " ") || return 1
	echo "packages: $names"
	get-myos -m $MIRROR $names
}
if [ "$MODE" = full ]; then
	t packages install_packages
fi

. $TESTS/kernel.sh
for f in $TESTS/ports/*.sh; do
	[ -f "$f" ] && . "$f"
done

echo "TESTS DONE $passed/$total"
