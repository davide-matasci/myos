# The Rust std on mlibc (mix/main.rs): a program of Rust and C that share
# the C library's heap, threads and thread-local storage; its arguments
# arrive, main's value is its exit status and the libc's atexit functions
# run (the OK line is printed from one).
std_mlibc() {
	out=$(/bin/std/mlibc/mix one two)
	rc=$?
	echo "$out"
	[ $rc = 42 ] && [ "$out" = "[ OK ] std mlibc" ]
}
t -t mlibc,std,sched std_mlibc std_mlibc
# Unwinding (unwind/main.rs): panics caught with their payloads, the
# frames' values dropped on the way, a lock poisoned, a panicking thread
# ending alone.
t -t mlibc,std,libcxx,unwind std_unwind /bin/std/mlibc/unwind
