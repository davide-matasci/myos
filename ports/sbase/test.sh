# sbase: one of its utilities runs and prints.
sbase_echo() {
	[ "$(/bin/sbase/echo hi)" = hi ]
}
t sbase_echo sbase_echo
