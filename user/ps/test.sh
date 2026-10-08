# ps (user/ps/ps): the shell running it, and a sleep it started under it;
# the kernel's processes only with -e; with -e -L, the idle tasks (pid 0)
# one thread per CPU.
ps_list() {
	sleep 30 &
	s=$!
	ps > /tmp/ps.out
	ps -e > /tmp/ps-e.out
	ps -e -L > /tmp/ps-eL.out
	kill $s
	cpus=$(grep -c '^cpu' /proc/cpu)
	idle=$(grep -c '^ *0 ' /tmp/ps-eL.out)
	grep -q "^ *$$ " /tmp/ps.out \
		&& grep -E -q "^ *$s +$$ .* sleep\$" /tmp/ps.out \
		&& ! grep -q '^ *0 ' /tmp/ps.out \
		&& grep -E -q '^ *0 +0 .* idle$' /tmp/ps-e.out \
		&& [ "$idle" = "$cpus" ] && return 0
	echo "pid $$, sleep $s, $cpus CPUs, $idle idle threads"
	cat /tmp/ps.out /tmp/ps-e.out /tmp/ps-eL.out
	return 1
}
t ps ps_list
