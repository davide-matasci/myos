# sbase: one of its utilities runs and prints.
sbase_echo() {
	[ "$(/bin/sbase/echo hi)" = hi ]
}
t sbase_echo sbase_echo

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
