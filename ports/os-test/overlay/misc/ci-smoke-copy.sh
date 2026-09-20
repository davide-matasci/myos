#!/bin/sh
# Thin writable staging for boot CI os-test smoke.
# Copies Makefile + misc/ + per-suite headers + each path in misc/ci-boot.tests
# (basic smoke + ~100 non-basic) into DEST — not the full /lib/os-test tree.
# Quote-free invocation for oksh:
#   sh /lib/os-test/misc/ci-smoke-copy.sh /tmp/o
#
# TESTLIST lines:
#   TESTS += pwd/setpwent          → basic/pwd/setpwent.c  (legacy basic form)
#   TESTS += limits/CHAR_BIT       → limits/CHAR_BIT.c     (suite-prefixed)
set -u
SRC=/lib/os-test
DEST=${1:-/tmp/o}

if [ ! -d "$SRC" ]; then
	echo "ci-smoke-copy: missing $SRC" >&2
	exit 1
fi

rm -rf "$DEST"
mkdir -p "$DEST/misc" || exit 1

cp "$SRC/Makefile" "$DEST/Makefile" || exit 1
cp -r "$SRC/misc/." "$DEST/misc/" || exit 1

for hdr in \
	basic/basic.h \
	limits/suite.h \
	io/io.h \
	malloc/malloc.h \
	paths/suite.h \
	process/process.h \
	signal/signal.h \
	stdio/suite.h \
	udp/suite.h
do
	if [ -f "$SRC/$hdr" ]; then
		mkdir -p "$DEST/${hdr%/*}" || exit 1
		cp "$SRC/$hdr" "$DEST/$hdr" || exit 1
	fi
done

LIST="$SRC/misc/ci-boot.tests"
if [ ! -f "$LIST" ]; then
	echo "ci-smoke-copy: missing $LIST" >&2
	exit 1
fi

# One-level include expansion: ci-boot.tests → ci-basic-smoke + ci-nonbasic-100.
: > /tmp/ci-smoke-lists
echo "$LIST" >> /tmp/ci-smoke-lists
while IFS= read -r line || [ -n "$line" ]; do
	case "$line" in
	include\ misc/*)
		inc=${line#include }
		while [ "${inc# }" != "$inc" ]; do inc=${inc# }; done
		[ -n "$inc" ] && echo "$SRC/$inc" >> /tmp/ci-smoke-lists
		;;
	esac
done < "$LIST"

staged=0
while IFS= read -r LISTF || [ -n "$LISTF" ]; do
	[ -z "$LISTF" ] && continue
	[ -f "$LISTF" ] || continue
	while IFS= read -r line || [ -n "$line" ]; do
		case "$line" in
		TESTS\ +=*)
			path=${line#TESTS +=}
			while [ "${path# }" != "$path" ]; do path=${path# }; done
			[ -z "$path" ] && continue

			suite=${path%%/*}
			rest=${path#*/}
			# Suite-prefixed when the first component is a top-level suite dir
			# that actually contains rest.c; else legacy basic-relative.
			if [ "$suite" != "$path" ] && [ -f "$SRC/$suite/$rest.c" ]; then
				src="$SRC/$suite/$rest.c"
				destpath="$suite/$rest"
			else
				src="$SRC/basic/$path.c"
				destpath="basic/$path"
			fi

			if [ ! -f "$src" ]; then
				echo "ci-smoke-copy: missing $src (from $path)" >&2
				exit 1
			fi
			mkdir -p "$DEST/${destpath%/*}" || exit 1
			cp "$src" "$DEST/$destpath.c" || exit 1
			staged=$((staged + 1))
			;;
		esac
	done < "$LISTF"
done < /tmp/ci-smoke-lists

echo "ci-smoke-copy: staged $DEST ($staged sources)"
exit 0
