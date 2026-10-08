# sbase: one of its utilities runs and prints.
sbase_echo() {
	[ "$(/bin/sbase/echo hi)" = hi ]
}
t sbase_echo sbase_echo

# One multicall ELF: the tool is the last component of argv[0], through a
# symlink under another directory too; a name that is no tool exits 127.
sbase_multicall() {
	rm -rf /tmp/sbm && mkdir /tmp/sbm || return 1
	ln -s /bin/sbase/echo /tmp/sbm/echo && ln -s /bin/sbase/echo /tmp/sbm/nope || return 1
	[ "$(/tmp/sbm/echo hi)" = hi ] || return 1
	out=$(/tmp/sbm/nope 2>&1)
	rc=$?
	echo "nope: $rc $out"
	[ $rc -eq 127 ] && [ "$out" = "sbase: not a tool name: nope" ] && rm -r /tmp/sbm
}
t sbase_multicall sbase_multicall

# printf's C99 length modifiers (newlib --enable-newlib-io-c99-formats):
# sbase wc counts with %zu.
sbase_wc() {
	n=$(printf abc | /bin/sbase/wc -c)
	echo "wc -c: $n"
	[ $n = 3 ]
}
t sbase_wc sbase_wc

# File times, on tmpfs: touch sets them (utimensat; 946782240 is
# 2000-01-02 03:04 UTC), creates a file at the current time (futimens) and
# copies another file's; a write moves the modification time on.
sbase_touch() {
	rm -f /tmp/tt1 /tmp/tt2
	/bin/sbase/touch -T 946782240 /tmp/tt1 && /bin/sbase/touch /tmp/tt2 || return 1
	l=$(/bin/sbase/ls -l /tmp/tt1)
	echo "ls: $l"
	case $l in *"Jan 02  2000"*) ;; *) return 1 ;; esac
	[ -n "$(/bin/sbase/find /tmp/tt2 -newer /tmp/tt1)" ] || return 1
	[ -z "$(/bin/sbase/find /tmp/tt1 -newer /tmp/tt2)" ] || return 1
	/bin/sbase/touch -r /tmp/tt1 /tmp/tt2 || return 1
	[ -z "$(/bin/sbase/find /tmp/tt2 -newer /tmp/tt1)" ] || return 1
	echo x >> /tmp/tt2
	[ -n "$(/bin/sbase/find /tmp/tt2 -newer /tmp/tt1)" ] && rm /tmp/tt1 /tmp/tt2
}
t sbase_touch sbase_touch

# The host name: hostname sets it (sethostname into the kernel's
# /proc/sys/kernel/hostname), uname -n and the file show it.
sbase_hostname() {
	old=$(/bin/sbase/hostname) || return 1
	/bin/sbase/hostname testhost || return 1
	n=$(/bin/sbase/uname -n)
	f=$(/bin/sbase/cat /proc/sys/kernel/hostname)
	/bin/sbase/hostname "$old"
	echo "uname -n: $n, file: $f, back: $(/bin/sbase/hostname)"
	[ "$n" = testhost ] && [ "$f" = testhost ] && [ "$(/bin/sbase/hostname)" = "$old" ]
}
t sbase_hostname sbase_hostname

# syslog: logger's message is a line on the console, and with -s on stderr.
sbase_logger() {
	out=$(/bin/sbase/logger -s -t tag hello 2>&1)
	echo "logger: $out"
	[ "$out" = "tag: hello" ]
}
t sbase_logger sbase_logger

# Setting the clock (clock_settime, kept until the next boot; the RTC keeps
# its time): date sets 2030-01-02 03:04 UTC and reads it back, then puts the
# time back a minute ahead (date sets whole minutes; ahead, so no file
# later looks older than one before).
sbase_date() {
	now=$(/bin/sbase/date +%s) || return 1
	/bin/sbase/date -u 010203042030 || return 1
	d=$(/bin/sbase/date -u +%Y%m%d%H%M)
	/bin/sbase/date -u $(/bin/sbase/date -u -d $((now + 60)) +%m%d%H%M%Y) || return 1
	echo "set: $d, back: $(/bin/sbase/date -u)"
	case $d in 20300102030[45]) ;; *) return 1 ;; esac
	[ $(/bin/sbase/date +%s) -ge $now ]
}
t sbase_date sbase_date
