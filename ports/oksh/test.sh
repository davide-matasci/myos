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
t -t smoke shell_nosuch sh_nosuch
t -t smoke shell_echo sh_echo
t -t smoke shell_pipe sh_pipe
t -t smoke shell_redirect sh_redirect
t -t smoke shell_which sh_which
t -t smoke shell_path sh_path

# A background command killed right after it was started dies at once: the
# kill mostly lands while the child is still the shell (its few
# milliseconds between the fork and the exec), whose handler for a fatal
# signal only set a flag; the exec dropped it, and `sleep 3 & kill $!`
# slept on (exec.myos.patch; the test runner's hung watch waited out a
# whole sleep that way, CI #2315). Twenty rounds, none over a second.
# Ten more rounds with a trap of the script's own on TERM (the child's
# handler for it is then the shell's until the exec, and the exec-time
# check has to put the default back before it takes the signal again).
# A background job's SIGINT stays ignored, as POSIX has it: one sent right
# after the start must not end the job, nor one sent once the command runs
# (the exec of the command put the shell's original disposition back, the
# default, and a SIGINT that came after it ended the job: jobs.myos.patch,
# CI #2351), a subshell's command included.
sh_kill_fresh() {
	n=0
	while [ $n -lt 30 ]; do
		[ $n -lt 20 ] || trap 'echo trapped' TERM
		s=$(date +%s)
		sleep 3 &
		p=$!
		kill $p
		wait $p 2> /dev/null
		e=$(date +%s)
		[ $((e - s)) -lt 2 ] || { trap - TERM; echo "round $n: the killed sleep lived $((e - s)) s"; return 1; }
		n=$((n + 1))
	done
	trap - TERM
	for job in 'sleep 5' '(sleep 5)'; do
		eval "$job &"
		p=$!
		kill -INT $p
		sleep 1
		kill -0 $p 2> /dev/null || { wait $p 2> /dev/null; echo "$job &: died of a SIGINT sent at its start (wait status $?)"; return 1; }
		kill -INT $p
		sleep 1
		kill -0 $p 2> /dev/null || { wait $p 2> /dev/null; echo "$job &: died of a SIGINT sent once it ran (wait status $?)"; return 1; }
		s=$(date +%s)
		kill $p
		wait $p 2> /dev/null
		st=$?
		e=$(date +%s)
		# The job, two seconds old, is the sleep itself: its TERM ends it at once.
		[ $((e - s)) -lt 2 ] && [ $st -eq 143 ] || { echo "$job &: killed after two seconds: status $st after $((e - s)) s"; return 1; }
	done
	return 0
}
t -t smoke,signal shell_kill_fresh sh_kill_fresh
