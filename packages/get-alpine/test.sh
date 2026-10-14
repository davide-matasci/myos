# get-alpine, installed as an app (the full list, which has the network and
# the Linux layer loaded): it bootstraps a root given with -r (apk-tools and
# what it needs, fetched and checked natively, recorded in ROOT/.get-alpine)
# and has apk, under the layer, install jq with its dependencies: apk's
# database lists them all, the bootstrap included, and busybox's trigger
# ran (the applet links). Then netpbm natively (-n), whose package metadata
# is long. Running what it installed is the Linux layer's test (lx_alpine in
# user/tests/kernel.sh).
get_alpine_fetch() {
	r=/tmp/get-alpine-test
	run-myos get-alpine -r $r jq || return 1
	for p in apk-tools; do
		[ -s $r/.get-alpine/pkgs/$p ] || { echo "no record of $p"; ls -a $r $r/.get-alpine; return 1; }
	done
	for p in apk-tools busybox musl oniguruma jq; do
		grep -q "^P:$p\$" $r/lib/apk/db/installed || { echo "apk does not have $p"; grep '^P:' $r/lib/apk/db/installed; return 1; }
	done
	grep -q '^jq$' $r/etc/apk/world || { echo "jq not in world"; cat $r/etc/apk/world; return 1; }
	[ -x $r/usr/bin/jq ] && ls $r/lib/ld-musl-*.so.1 > /dev/null || { ls -R $r | head -40; return 1; }
	[ -L $r/bin/ls ] || { echo "no busybox links"; ls -la $r/bin | head; return 1; }
	# Every root gets the CAs, for a TLS client's default trust store.
	[ -s $r/etc/ssl/certs/ca-certificates.crt ] || { ls -R $r/etc; return 1; }
	# Installed packages are skipped: nothing is fetched or unpacked again.
	run-myos get-alpine -r $r jq > /tmp/get-alpine-again.out 2>&1 || return 1
	cat /tmp/get-alpine-again.out
	! grep -q -e '^get-alpine: bootstrapping' -e 'Installing' /tmp/get-alpine-again.out || return 1
	# The native unpack (-n, what alpine-disk.sh does on the host) of a
	# package whose .PKGINFO runs past 8 KiB (netpbm: a line for each of the
	# 370 programs it provides), with its datahash on the last line.
	run-myos get-alpine -n -r $r netpbm || return 1
	[ -s $r/.get-alpine/pkgs/netpbm ] && [ -x $r/usr/bin/anytopnm ] || { ls -a $r/.get-alpine/pkgs; return 1; }
	rm -r $r
}
t get_alpine get_alpine_fetch
