# get-alpine, installed as an app (the full list, which has the network): it
# fetches jq with its dependencies from Alpine's mirror into a root given
# with -r, checks them, and records them in ROOT/.get-alpine; nothing of
# Alpine's runs here (that is the Linux layer's test, lx_alpine in
# user/tests/kernel.sh, with the module loaded).
get_alpine_fetch() {
	r=/tmp/get-alpine-test
	run-myos get-alpine -r $r jq || return 1
	for p in musl oniguruma jq; do
		[ -s $r/.get-alpine/pkgs/$p ] || { echo "no record of $p"; ls -a $r $r/.get-alpine; return 1; }
	done
	[ -x $r/usr/bin/jq ] && ls $r/lib/ld-musl-*.so.1 > /dev/null || { ls -R $r | head -40; return 1; }
	# Installed packages are skipped.
	run-myos get-alpine -r $r jq > /tmp/get-alpine-again.out 2>&1 || return 1
	cat /tmp/get-alpine-again.out
	! grep -q '^get-alpine: jq ' /tmp/get-alpine-again.out || return 1
	rm -r $r
}
t get_alpine get_alpine_fetch
