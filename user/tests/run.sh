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
# (kernel.sh), which run first. The `packages` test installs the packages
# of the mirror the host serves that the selected tests use (every one of
# them for the whole list), and the packages' tests come with them.
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

# hung_watch NAME (in the background, beside test NAME): once the test has
# run HUNG seconds, its system as it is, on the console: a `HUNG` line, then
# each thread's /proc line (docs/proc.md: its state and the key it waits
# on) and each process's command line. A test that hangs is shown before
# the host's stall watchdog ends the boot (180 s of silence in a quick run,
# 600 s in one with the `heavy` tests, which may take minutes): CI #2295's
# lynx test stalled with every CPU idle, and nothing told what waited.
#
# The test's end reaches the watch through two files, not a signal: a TERM
# sent to the subshell while it was still setting up was lost (oksh reads a
# signal's old disposition by setting SIG_IGN first, and a TERM in between
# is dropped) or taken only once its `wait` had looked at the traps and
# before it slept (the kernel then ran a handler at the task's next syscall
# exit, issue #647; now at once), and `t` waited out the whole sleep: a
# trivial test took 300 s (CI #2315). `hung_watch_done` marks $OUT/hw-NAME.done and ends the
# sleep whose pid the watch leaves in $OUT/hw-NAME.pid; the watch writes the
# pid before it looks at the mark, so one side or the other sees the other's.
# (The sleep's own kill was lost too when it fell in the child's moment as
# a shell before the exec, to the handler there, which now takes it at
# once: ports/oksh/trap.myos.patch.)
hung_watch() {
	HUNG=300
	has "$SKIP" heavy && HUNG=120
	sleep $HUNG &
	hw_sleep=$!
	echo $hw_sleep > $OUT/hw-$1.pid
	if [ -s $OUT/hw-$1.done ]; then
		kill $hw_sleep 2> /dev/null
		return 0
	fi
	wait $hw_sleep 2> /dev/null
	: > $OUT/hw-$1.pid
	[ -s $OUT/hw-$1.done ] && return 0
	echo "HUNG $1: still running after ${HUNG}s; its threads (tid pid name state cpu wait time start pending blocked ignored caught), then the processes:"
	for hw_f in /proc/[0-9]*/task/*/status; do cat "$hw_f"; done 2> /dev/null
	for hw_d in /proc/[0-9]*; do echo "$hw_d $(tr '\0' ' ' < $hw_d/cmdline 2> /dev/null)"; done
}

# hung_watch_start NAME / hung_watch_done NAME: around the test, in the
# runner's shell (`wait` on the watch after `done` returns at once).
hung_watch_start() {
	: > $OUT/hw-$1.done
	: > $OUT/hw-$1.pid
	hung_watch $1 >&3 2>&1 &
}

hung_watch_done() {
	echo 1 > $OUT/hw-$1.done
	hw_pid=
	read -r hw_pid < $OUT/hw-$1.pid 2> /dev/null
	[ -n "$hw_pid" ] && kill $hw_pid 2> /dev/null
	return 0
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

# disk_apps NAME...: the packages NAME installed as apps on the scratch
# disk, under $DISK_APPS (run-myos -r $DISK_APPS NAME ...), for the tests
# tagged `disk`: /tmp/apps is a tmpfs, whose files are the kernel heap's
# (1 GiB at most for everything in it, 128 MiB a file), and a compiler is
# a hundred MiB and more. The disk is formatted here unless a test before
# left it mounted at /tmp/disk. get-myos's lines go to the console (fd
# 3): the host ends a boot that is silent for 600 s.
DISK_APPS=/tmp/disk/apps
# The kernel heap the tests need free once the packages are in /tmp/apps,
# in MiB (install_packages).
HEAP_SPARE=256
disk_apps() {
	if ! grep -q " /tmp/disk ext2 " /proc/mounts; then
		mkdir -p /tmp/disk && mkfs.ext2 /dev/nvme1n1/data > /dev/null \
			&& mount /dev/nvme1n1/data /tmp/disk ext2 || return 1
	fi
	for da_name in "$@"; do
		[ -e $DISK_APPS/$da_name/app ] || { get-myos -r $DISK_APPS -m $MIRROR "$@" 2>&3; return; }
	done
}

# shell_intact WHEN: the runner's own shell still passes "$@" on as words.
# CI #2639 and #2641 (x86): from some test on, every test given a command
# with arguments failed with `/bin/etc/pty_smokeu2: not found`: the words
# of "$@" joined by a `u`, which is the shell's first IFS character gone
# wrong while IFS itself still split words. Forty tests failed for one
# fault and none said where it began. The check runs before and after
# each test; a shell found broken says so once, with what IFS holds, and
# IFS is set again (which puts that character back), so that the tests
# that follow are tested.
shell_words() { [ $# -eq 2 ]; }
shell_pass() { shell_words "$@"; }
shell_join() { echo "\"\$@\" of a b: $# word(s), the first: $1"; }
shell_show() { shell_join "$@"; }
shell_intact() {
	shell_pass a b && return 0
	echo "the runner's shell no longer passes \"\$@\" on as words ($1)"
	shell_show a b
	echo "IFS: ${#IFS} byte(s): $(printf %s "$IFS" | od -t x1 | head -n 2 | tr '\n' ' ')"
	# What else the shell holds beside that character (the terminal's
	# width is the next word of its memory), and when: its pid, the last
	# background job's, the time since boot.
	echo "COLUMNS=${COLUMNS-unset} pid $$ last job ${!-none} up $(cut -d' ' -f1 /proc/uptime 2> /dev/null)"
	IFS=' 	
'
	shell_pass a b || echo "and setting IFS again did not mend it"
	return 1
}
# shell_phase WHEN: the same check where no test runs, at the runner's
# start and after its first pass over the tests' files: a shell that
# starts broken (an exec's fault), one that the pass breaks (its own
# work: functions defined, the lists read) and one that a test breaks
# are three different bugs. Said on the console, and the first test
# fails for it.
shell_phase() {
	shell_intact "found so $1" > $OUT/shell.phase && return 0
	sed 's/^/SHELL: /' $OUT/shell.phase
	SHELL_BAD=1
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
	t_pre=0
	shell_intact "found so before $t_name" > $OUT/$t_name.pre || t_pre=1
	if [ -n "$SHELL_BAD" ]; then
		cat $OUT/shell.phase >> $OUT/$t_name.pre
		t_pre=1
		SHELL_BAD=
	fi
	hung_watch_start $t_name
	t_hung=$!
	capture $OUT/$t_name.out "$@"
	t_rc=$?
	hung_watch_done $t_name
	wait $t_hung 2> /dev/null
	[ $t_pre -eq 0 ] || cat $OUT/$t_name.pre >> $OUT/$t_name.out
	if ! shell_intact "after $t_name" >> $OUT/$t_name.out || [ $t_pre -ne 0 ]; then
		[ $t_rc -ne 0 ] || t_rc=1
	fi
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

# wanted_packages NAMES: of the mirror's packages NAMES, the ones the
# selected `pkg` tests use: those their tags name (a package's tests carry
# its name; a test of the kernel or of another port names what it runs,
# alpine_jq get-alpine, get_myos_list st). A `pkg` test that names none
# uses the mirror itself (get-myos's into roots of their own), and so does
# a `disk` one: its package is too large for /tmp/apps (disk_apps) and the
# test installs it on the scratch disk. In the order
# of NAMES; what they need get-myos installs with them.
wanted_packages() {
	w_want=
	while read -r w_name w_tags; do
		has "$w_tags" pkg || continue
		has "$w_tags" disk && continue
		for p in $1; do
			has "$w_tags" "$p" && w_want="$w_want,$p"
		done
	done < $OUT/selected
	w_out=
	for p in $1; do
		has "$w_want" "$p" && w_out="$w_out $p"
	done
	echo "${w_out# }"
}

# The packages: those of this build from the host's mirror that the
# selected tests use (wanted_packages; docs/packages.md: packages.txt names
# the ports the image lacks, the index has the image's ports too), as apps
# under /tmp/apps (all of them do not fit the boot disk's data partition):
# the packages' tests run them with run-myos. The os-test boot installs
# os-test (and make, which it needs), the Alpine one get-alpine, the
# packages' boot the packages with tests of their own and what those use
# (not os-test's 60 MB, nor get-alpine).
install_packages() {
	all=$(curl -fsS $MIRROR/packages.txt | tr "\n" " ") || return 1
	names=$(wanted_packages "$all")
	echo "packages: ${names:-none}"
	[ -n "$names" ] || return 0
	# The first package with dependencies alone first, on the fresh system:
	# get-myos must install them with it (the deps field of the index).
	first=
	get-myos -m $MIRROR -l > /tmp/get-myos-list || return 1
	while read -r name version deps state; do
		case "$name:$deps" in
		mirror:* | system:* | *:- | *:) ;;
		*)
			case " $names " in
			*" $name "*) first=$name; break ;;
			esac
			;;
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
	# get-myos's line per package (stderr) goes to the console: the install of
	# them all is minutes under TCG (python's alone is minutes on
	# riscv64), and the host's stall watchdog wants the console moving.
	get-myos -m $MIRROR $names 2>&3 || return 1
	# /tmp/apps is a tmpfs: the apps are in the kernel heap, with what the
	# tests' programs then take of it (WebKit most). CI #2652: with llvm's
	# 120 MiB among them the aarch64 boot had 30 MiB left when surf ran,
	# under the reserve a fork needs, and every test after it failed to
	# fork. What is left is said on the console, and too little fails
	# here, by name, not in whichever test meets the end of the heap.
	pk_free=$(sed -n 's/^HeapFreeKiB: *//p' /proc/meminfo)
	pk_size=$(sed -n 's/^HeapKiB: *//p' /proc/meminfo)
	echo "packages: $((pk_free / 1024)) MiB of the kernel heap's $((pk_size / 1024)) free with the apps in /tmp/apps" >&3
	if [ "$pk_free" -lt $((HEAP_SPARE * 1024)) ]; then
		echo "the apps leave $((pk_free / 1024)) MiB of the kernel heap, under the $HEAP_SPARE the tests need:"
		echo "a package this large installs on the scratch disk (a \`disk\` test, disk_apps)"
		return 1
	fi
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
SHELL_BAD=
shell_phase "at the runner's start"
LISTING=1
source_tests
LISTING=
shell_phase "after the first pass over the tests' files"
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
# What the run left of the kernel heap (tmpfs and the tasks live in it):
# beside the packages' line above, how much the tests themselves took.
echo "kernel heap: $(($(sed -n 's/^HeapFreeKiB: *//p' /proc/meminfo) / 1024)) MiB free at the end"
# netfs's conversation slots still in use once the tests are over: the
# boot's own processes (dropbear's listener) and what a test left behind,
# or a slot netfs lost track of (128 of those, and socket() fails with
# EIO: issue #574, the listen smoke's second socket twice). A few lines.
if [ -r /net/slots ]; then
	echo "netfs slots in use: $(grep -c '^[0-9]' /net/slots)"
	cat /net/slots
fi
