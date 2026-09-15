#!/bin/sh
# Thin writable staging for boot CI os-test smoke.
# Copies only Makefile + misc/ + basic/basic.h + each path in
# misc/ci-basic-smoke.tests (+ matching prebuilt/ ELFs when present)
# into DEST — not the full /lib/os-test tree (full `cp -r` was slow enough
# under unfinished SMP to burn the 90m window; see CI #866). Quote-free
# invocation for oksh:
#   sh /lib/os-test/misc/ci-smoke-copy.sh /tmp/o
set -u
SRC=/lib/os-test
DEST=${1:-/tmp/o}

if [ ! -d "$SRC" ]; then
	echo "ci-smoke-copy: missing $SRC" >&2
	exit 1
fi

rm -rf "$DEST"
mkdir -p "$DEST/misc" "$DEST/basic" || exit 1

cp "$SRC/Makefile" "$DEST/Makefile" || exit 1
# misc/ holds the harness, TESTLIST, and this script.
cp -r "$SRC/misc/." "$DEST/misc/" || exit 1
cp "$SRC/basic/basic.h" "$DEST/basic/basic.h" || exit 1

LIST="$SRC/misc/ci-basic-smoke.tests"
if [ ! -f "$LIST" ]; then
	echo "ci-smoke-copy: missing $LIST" >&2
	exit 1
fi

# Lines look like: TESTS += pwd/setpwent
# Avoid dirname/sed — keep portable for oksh on the guest.
while IFS= read -r line || [ -n "$line" ]; do
	case "$line" in
	TESTS\ +=*)
		path=${line#TESTS +=}
		# trim leading spaces
		while [ "${path# }" != "$path" ]; do path=${path# }; done
		[ -z "$path" ] && continue
		src="$SRC/basic/$path.c"
		if [ ! -f "$src" ]; then
			echo "ci-smoke-copy: missing $src" >&2
			exit 1
		fi
		case "$path" in
		*/*)
			dir=${path%/*}
			mkdir -p "$DEST/basic/$dir" || exit 1
			;;
		*)
			mkdir -p "$DEST/basic" || exit 1
			;;
		esac
		cp "$src" "$DEST/basic/$path.c" || exit 1
		# Prebuilt ELFs are exec'd in place from the read-only initramfs
		# (misc/myos-run.sh falls back to $SRC/prebuilt) — copying 18 MB of
		# binaries into tmpfs fork-by-fork stalls the TCG boot window.
		;;
	esac
done < "$LIST"

echo "ci-smoke-copy: staged $DEST"
exit 0
