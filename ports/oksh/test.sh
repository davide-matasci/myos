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
t shell_nosuch sh_nosuch
t shell_echo sh_echo
t shell_pipe sh_pipe
t shell_redirect sh_redirect
t shell_which sh_which
