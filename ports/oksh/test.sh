# oksh: the shell every boot logs into.
sh_nosuch() {
	nosuchcmd 2>&1 | grep -q "not found"
}
sh_echo() {
	[ "$(echo test)" = test ]
}
sh_pipe() {
	[ "$(echo pipe | cat)" = pipe ]
}
# The redirect uses newlib O_CREAT; it must create on the tmpfs.
sh_redirect() {
	echo test > /tmp/aaa && [ "$(cat /tmp/aaa)" = test ]
}
# `which` walks $PATH with fstatat: an absolute hit, never "not an external command".
sh_which() {
	w=$(which ls)
	echo "$w"
	case $w in
	/*/ls) return 0 ;;
	esac
	return 1
}
# The shell keeps the PATH it inherits (a script run with its own directory
# first finds its programs there), and has myos's directories without one.
sh_path() {
	mkdir -p /tmp/pathbin
	printf '#!/bin/sh\necho mine\n' > /tmp/pathbin/path-probe
	chmod +x /tmp/pathbin/path-probe
	[ "$(PATH=/tmp/pathbin:$PATH sh -c path-probe)" = mine ] || return 1
	p=$(env -i /bin/sh -c 'echo $PATH')
	echo "$p"
	[ "$p" = /bin/sbase:/bin/coreutils:/bin/ubase:/bin/custom:/bin/tcc:/bin/std:/bin/etc ]
}
t shell_nosuch sh_nosuch
t shell_echo sh_echo
t shell_pipe sh_pipe
t shell_redirect sh_redirect
t shell_which sh_which
t shell_path sh_path
