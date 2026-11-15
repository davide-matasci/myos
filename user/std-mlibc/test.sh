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
# Rust shared objects (dylib/): a program over a Rust dylib that holds
# std loads another with dlopen; one std and heap between them, the
# plugin's panic caught in the program, its thread-locals per thread.
t -t mlibc,std,libcxx,unwind,dl std_dylib /bin/std/mlibc/dylib /bin/std/mlibc/libplugin.so
# The flavour is a unix target (unix/main.rs): the environment, 40
# arguments with one of 3000 bytes, children with pipes, a status and a
# signal, files with their modes, links and locks, TCP, UDP and unix
# sockets over the loopback, time and named threads.
std_unix() {
	out=$(/bin/std/mlibc/unix)
	rc=$?
	echo "$out"
	[ $rc = 0 ] && [ "$out" = "[ OK ] std unix" ]
}
t -t mlibc,std,libc,exec,netfs,fs std_unix std_unix
