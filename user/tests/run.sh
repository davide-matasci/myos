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
# of the mirror the host serves. `reboot` is the full test's second boot,
# of the disk the first one upgraded (get-myos --upgrade): it checks only
# that the system came up from the slot the upgrade wrote; `installed` the
# third, of the disk get-myos --install made: up from its slot a.

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

# The host watches the serial port; copying every line to a big screen too
# makes a chatty test several times slower under emulation. The screen
# comes back at the end.
mirror=$(grep '^mirror ' /dev/console/ctl 2>/dev/null)
[ -n "$mirror" ] && echo 'mirror off' > /dev/console/ctl

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

# t_last NAME: run the test NAME (a function) after every other one: the
# tests that leave something on the scratch disk the host boots afterwards,
# which a later test would reuse (mkfs.ext2's formats the whole disk).
LAST=
t_last() {
	LAST="$LAST $1"
}

# contains NEEDLE FILE: the file has the string somewhere.
contains() {
	grep -q -F -- "$1" "$2"
}

# The second boot: slot b, with b's release (its version file, which the
# upgrade wrote, is this initramfs's /lib/myos-release), boots by default.
# The third, of the disk get-myos --install made: its slot a, likewise.
# Either has its ESP at /boot and the data partition its fstab names at
# /data; the second one also the app the first installed there.
booted_slot() {
	cat /proc/cmdline /lib/myos-release /proc/mounts
	grep -q "slot=$1" /proc/cmdline || return 1
	[ "$(cat /boot/boot/$1/version)" = "$(cat /lib/myos-release)" ] || { echo "/boot has no slot $1 at this release"; return 1; }
	# The disk's own data partition at /data (its /boot/fstab).
	data=$(grep ' /data ext2 ' /proc/mounts | cut -d' ' -f1)
	g=$(grep "^${data#/dev/} " /proc/partitions | cut -d' ' -f5)
	[ -n "$g" ] && grep -q "^PARTUUID=$g /data ext2\$" /boot/fstab
}
if [ "$MODE" = reboot ] || [ "$MODE" = installed ]; then
	if [ "$MODE" = reboot ]; then
		t booted_slot_b booted_slot b
		# The app the first boot installed on the data partition
		# (user/get-myos/test.sh's apps_data) runs as it did: an app is
		# its directory, nothing to redo after a reboot.
		t app_kept sh -c '[ "$(run-myos clear -T linux)" = "$(printf "\033[H\033[J")" ]'
	else
		t booted_installed booted_slot a
	fi
	[ -n "$mirror" ] && echo "$mirror" > /dev/console/ctl
	echo "TESTS DONE $passed/$total"
	exit 0
fi

# The full boot gets every package of this build from the host's mirror
# first (docs/packages.md; packages.txt names the ports the image lacks,
# the index has the image's ports too), as apps under /tmp/apps (all of
# them do not fit the boot disk's data partition): the packages' tests,
# which come with them, run them with run-myos.
install_packages() {
	names=$(curl -fsS $MIRROR/packages.txt | tr "\n" " ") || return 1
	echo "packages: $names"
	# The first package with dependencies alone first, on the fresh system:
	# get-myos must install them with it (the deps field of the index).
	first=
	get-myos -m $MIRROR -l > /tmp/get-myos-list || return 1
	while read -r name version deps state; do
		case "$name:$deps" in
		mirror:* | system:* | *:- | *:) ;;
		*) first=$name; break ;;
		esac
	done < /tmp/get-myos-list
	if [ -n "$first" ]; then
		deps=$(echo "$deps" | tr "," " ")
		echo "$first needs: $deps"
		get-myos -m $MIRROR $first || return 1
		for d in $deps; do
			[ -s $MYOS_APPS/.get-myos/pkgs/$d ] || { echo "missing dependency $d"; return 1; }
		done
	fi
	get-myos -m $MIRROR $names
}
if [ "$MODE" = full ]; then
	export MYOS_APPS=/tmp/apps
	t packages install_packages
fi

# The image's tests, then the installed apps' (each app has its port's at
# lib/myos-tests/ports/), all in the order of their names.
. $TESTS/kernel.sh
for f in $TESTS/ports/*.sh ${MYOS_APPS:-/nonexistent}/*/lib/myos-tests/ports/*.sh; do
	[ -f "$f" ] && echo "${f##*/} $f"
done | sort | cut -d' ' -f2 > /tmp/myos-tests-list
for f in $(cat /tmp/myos-tests-list); do
	. "$f"
done
for f in $LAST; do
	t $f $f
done

[ -n "$mirror" ] && echo "$mirror" > /dev/console/ctl
echo "TESTS DONE $passed/$total"
