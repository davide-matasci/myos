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
