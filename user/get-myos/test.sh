# get-myos: the image's release file, and in the full mode (the host serves
# this build's packages) the index's header against it, the listing with
# the installed marks, and an upgrade: a package recorded at a stale
# version is fetched again by -u and recorded at the index's. The
# dependency install itself is the `packages` test of run.sh, on the fresh
# system.
release_file() {
	cat /lib/myos-release
	grep -q '^release=[0-9]* commit=[0-9a-f]* abi=[0-9][0-9]*$' /lib/myos-release
}
t release release_file

get_myos_list() {
	get-myos -m $MIRROR -l > /tmp/get-myos-list || return 1
	cat /tmp/get-myos-list
	abi=$(sed 's/.*abi=//' /lib/myos-release)
	grep -q "^mirror: release [0-9]* abi $abi\$" /tmp/get-myos-list || return 1
	grep -q "^system: release [0-9]* abi $abi\$" /tmp/get-myos-list || return 1
	# Every package of the mirror was installed by the packages test, st
	# with its dependencies.
	! grep -q ' -$' /tmp/get-myos-list || return 1
	grep -q '^st [0-9a-f]* x11-xft,x11-fonts installed$' /tmp/get-myos-list
}
get_myos_upgrade() {
	echo stale > /tmp/pkg/var/lib/get-myos/pkgs/lua
	get-myos -m $MIRROR -u || return 1
	have=$(cat /tmp/pkg/var/lib/get-myos/pkgs/lua)
	want=$(get-myos -m $MIRROR -l | sed -n 's/^lua \([0-9a-f]*\) .*/\1/p')
	echo "lua: recorded $have, index $want"
	[ -n "$want" ] && [ "$have" = "$want" ] && lua -v
}
if [ "$MODE" = full ]; then
	t get_myos_list get_myos_list
	t get_myos_upgrade get_myos_upgrade
fi
