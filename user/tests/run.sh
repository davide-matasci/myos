#!/bin/sh
# The boot tests (docs/testing.md). `myos test` boots an image, logs in and
# types
#
#   sh /lib/myos-tests/run.sh [--tags TAG,...] [--skip TAG,...]
#
# which anyone can do by hand at the prompt too. Every test carries tags
# (what it needs: `net`, `pkg`, `heavy`, `install`; what it covers: a
# module's or a port's name; and its own name); the selection runs the
# tests with any of the --tags (all of them without --tags), minus those
# with any of the --skip tags, so `--tags dns` runs that one test. `mini`
# and `full` stand for the two old lists: `--skip net,pkg,heavy,install`
# and everything. One line per
# test on the console, which the host watches on the serial port:
#
#   TEST <name> PASS
#   TEST <name> FAIL (exit N)     then the test's output, indented
#   HOST <request...>             a request to the host (connect to a port)
#   TESTS DONE <passed>/<total>   at the end, then TESTS SKIPPED <n>
#
# A test is a command (mostly a shell function); its exit status decides,
# its output goes to /tmp/myos-tests/<name>.out and is shown when it fails.
# Every test belongs to the port of what it tests (ports/*.sh, from their
# PORT_TEST; the packer names them so the core image ports come first, the
# other image ports next, the packages last), except the kernel's own
# (kernel.sh), which run first. The `packages` test installs every package
# of the mirror the host serves, and the packages' tests come with them.
# `reboot` is the second boot of a run that upgraded the boot disk
# (get-myos --upgrade): it checks only that the system came up from the
# slot the upgrade wrote; `installed` the third, of the disk get-myos
# --install made: up from its slot a.

MODE=tests
TAGS=
SKIP=
while [ $# -gt 0 ]; do
	case $1 in
	mini) SKIP=net,pkg,heavy,install ;;
	full | tests) ;;
	reboot | installed) MODE=$1 ;;
	--tags) TAGS=$2; shift ;;
	--skip) SKIP=$2; shift ;;
	*) echo "run.sh: bad argument: $1" >&2; exit 2 ;;
	esac
	shift
done
TESTS=/lib/myos-tests
OUT=/tmp/myos-tests
MIRROR=${MYOS_MIRROR:-http://10.0.2.2:8765}
passed=0
total=0
skipped=0
mkdir -p $OUT

# The boot script (/etc/rc, docs/install.md) runs in the background, past
# the login prompt: the tests check what it set up, so it must be over
# (it says so in /tmp/boot.done).
i=0
while [ $i -lt 300 ] && ! [ -f /tmp/boot.done ]; do
	sleep 1
	i=$((i + 1))
done

# fd 3 stays the console whatever a test redirects: the HOST requests and a
# long test's progress go there.
exec 3>&1

# capture FILE COMMAND...: the command's output, both streams, in the file.
capture() {
	f=$1
	shift
	"$@" > "$f" 2>&1
}

# has LIST ITEM: the comma list has the item.
has() {
	case ",$1," in
	*",$2,"*) return 0 ;;
	esac
	return 1
}

# selected NAME TAGS: whether the selection takes the test (TAGS a comma
# list, the file's own tags included; the name is a tag of its own).
selected() {
	for tag in $(echo "$2,$1" | tr , ' '); do
		has "$SKIP" "$tag" && return 1
	done
	[ -z "$TAGS" ] && return 0
	for tag in $(echo "$2,$1" | tr , ' '); do
		has "$TAGS" "$tag" && return 0
	done
	return 1
}

# t [-t TAGS] NAME COMMAND...: run one test, if the selection takes it.
# FILE_TAGS are the tags of the file being sourced (its port's name, `pkg`
# for a package's), on every test of it. In the first pass over the files
# (LISTING set) nothing runs: the selected tests' names and tags are
# listed, for what the run needs before the tests (the packages).
t() {
	t_tags=$FILE_TAGS
	if [ "$1" = -t ]; then
		t_tags="$2,$t_tags"
		shift 2
	fi
	t_name=$1
	shift
	if [ -z "$FORCE" ] && ! selected "$t_name" "$t_tags"; then
		[ -n "$LISTING" ] || skipped=$((skipped + 1))
		return 0
	fi
	if [ -n "$LISTING" ]; then
		echo "$t_name $t_tags" >> $OUT/selected
		return 0
	fi
	total=$((total + 1))
	# The test runs in this shell (a function sets what later ones read):
	# the runner's own variables are named so that none clobbers them.
	capture $OUT/$t_name.out "$@"
	t_rc=$?
	if [ $t_rc -eq 0 ]; then
		passed=$((passed + 1))
		echo "TEST $t_name PASS"
	else
		echo "TEST $t_name FAIL (exit $t_rc)"
		tail -n 40 $OUT/$t_name.out | sed 's/^/    /'
	fi
}

# t_last [-t TAGS] NAME: run the test NAME (a function) after every other
# one: the tests that leave something on the scratch disk the host boots
# afterwards, which a later test would reuse (mkfs.ext2's formats the whole
# disk).
LAST=
t_last() {
	t_tags=$FILE_TAGS
	if [ "$1" = -t ]; then
		t_tags="$2,$t_tags"
		shift 2
	fi
	LAST="$LAST $1:$t_tags"
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

# The --sync /etc/rc started in the background: done within two minutes.
apps_synced() {
	log=/tmp/get-myos-sync.log
	for i in $(seq 60); do
		grep -q '^get-myos: done$' $log 2> /dev/null && break
		sleep 2
	done
	cat $log
	grep -q '^get-myos: done$' $log && [ "$(cat /data/apps/.get-myos/generations/current)" = 1 ]
}

FILE_TAGS=
if [ "$MODE" = reboot ] || [ "$MODE" = installed ]; then
	if [ "$MODE" = reboot ]; then
		t booted_slot_b booted_slot b
		# The app the first boot installed on the data partition
		# (user/get-myos/test.sh's apps_data; the host skips this
		# test when that one did not run) runs as it did: an app is
		# its directory, nothing to redo after a reboot.
		t app_kept sh -c '[ "$(run-myos clear -T linux)" = "$(printf "\033[H\033[J")" ]'
		# /etc/rc ran `get-myos --sync` for the list apps_data left
		# (/data/apps/apps.list), with the mirror that install used:
		# nothing to change, so it ends at the same generation.
		t -t app_kept apps_synced apps_synced
		# apps_data made clear global: /etc/rc bound it again, on PATH.
		t -t app_kept app_global sh -c '[ "$(/bin/custom/clear -T linux)" = "$(printf "\033[H\033[J")" ]'
	else
		t booted_installed booted_slot a
	fi
	echo "TESTS DONE $passed/$total"
	exit 0
fi

echo "tests: tags=${TAGS:--} skip=${SKIP:--}"

# wait_lease: netd's DHCP lease (/net/ndb), within ten seconds of the
# first test: a connect before it fails at once (no address to send
# from), and the packages test is the run's first.
wait_lease() {
	i=0
	while [ ! -s /net/ndb ] && [ $i -lt 10 ]; do
		sleep 1
		i=$((i + 1))
	done
}

# The packages: every one of this build from the host's mirror
# (docs/packages.md; packages.txt names the ports the image lacks, the
# index has the image's ports too), as apps under /tmp/apps (all of them
# do not fit the boot disk's data partition): the packages' tests run
# them with run-myos.
install_packages() {
	wait_lease
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
		# Each at the version the index names (dep@version).
		for d in $deps; do
			[ "$(cat $MYOS_APPS/.get-myos/pkgs/${d%@*} 2> /dev/null)" = "${d#*@}" ] \
				|| { echo "missing dependency $d"; return 1; }
		done
	fi
	get-myos -m $MIRROR $names
}
# source_tests: the kernel's tests, then every port's (the packages' are
# in the image too), in the order of their names. A ports file is
# `<group>-<name>.sh`: its tests are tagged with the port's name, a
# package's (group 2) with `pkg` too; the deferred ones (t_last) last.
ls $TESTS/ports/*.sh | sort > /tmp/myos-tests-list
source_tests() {
	LAST=
	FILE_TAGS=
	. $TESTS/kernel.sh
	for f in $(cat /tmp/myos-tests-list); do
		name=${f##*/}
		name=${name%.sh}
		FILE_TAGS=${name#*-}
		case $name in
		2-*) FILE_TAGS=$FILE_TAGS,pkg ;;
		esac
		. "$f"
	done
	FILE_TAGS=
	for f in $LAST; do
		t -t "${f#*:}" "${f%%:*}" "${f%%:*}"
	done
}

# The first pass lists the selected tests; a `pkg` one among them needs the
# packages installed as apps: the `packages` test runs before the others,
# whatever the selection says (its failure fails them on their own). Then
# the second pass runs them.
rm -f $OUT/selected
LISTING=1
source_tests
LISTING=
if [ -s $OUT/selected ]; then
	echo "selected: $(wc -l < $OUT/selected) tests"
fi
if cut -d' ' -f2 $OUT/selected 2>/dev/null | grep -q -E "(^|,)pkg(,|$)"; then
	export MYOS_APPS=/tmp/apps
	FORCE=1
	t -t net,pkg packages install_packages
	FORCE=
fi
source_tests

echo "TESTS DONE $passed/$total"
echo "TESTS SKIPPED $skipped"
