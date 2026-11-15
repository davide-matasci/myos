# llvm, installed as an app on the scratch disk (`disk`, the runner's
# disk_apps: 120 MiB that the kernel heap under /tmp/apps does not have
# to spare beside the other packages): clang and lld on myos build
# programs for it. In CI's `disk` boot, with the rustc package's tests.
#
# A C program compiled, linked (a static PIE against mlibc, by the
# driver's own link: lld is the same program under another name) and run.
llvm_c() {
	disk_apps llvm || return 1
	cat > /tmp/llvm-hello.c <<'EOT'
#include <stdio.h>
#include <string.h>
int main(int argc, char **argv) {
#ifndef __myos__
	return 2;
#endif
	printf("hello from %s, %zu\n", argv[1], strlen(argv[1]));
	return argc == 2 ? 0 : 1;
}
EOT
	run-myos -r $DISK_APPS llvm -O2 -Wall -Werror -o /tmp/llvm-hello /tmp/llvm-hello.c || return 1
	out=$(/tmp/llvm-hello clang)
	echo "$out"
	[ "$out" = "hello from clang, 5" ]
}
t -t net,disk,exec llvm_c llvm_c

# C++ against libc++: a container, a string stream and an exception
# caught (libc++abi and libunwind in the link, the frames found through
# the program's PT_GNU_EH_FRAME).
llvm_cxx() {
	disk_apps llvm || return 1
	cat > /tmp/llvm-hello.cc <<'EOT'
#include <map>
#include <sstream>
#include <stdexcept>
#include <string>
#include <cstdio>
static int twice(int n) {
	if (n < 0)
		throw std::out_of_range("negative");
	return 2 * n;
}
int main() {
	std::map<std::string, int> m{{"a", twice(1)}, {"b", twice(20)}};
	std::ostringstream s;
	for (auto &[k, v] : m)
		s << k << '=' << v << ' ';
	try {
		twice(-1);
	} catch (const std::exception &e) {
		s << e.what();
	}
	std::puts(s.str().c_str());
	return 0;
}
EOT
	run-myos -r $DISK_APPS llvm:clang++ -std=c++20 -O1 -o /tmp/llvm-hello-cc /tmp/llvm-hello.cc || return 1
	out=$(/tmp/llvm-hello-cc)
	echo "$out"
	[ "$out" = "a=2 b=40 negative" ]
}
t -t net,disk,exec llvm_cxx llvm_cxx

# One compiler for the three machines: an object for each arch, which the
# binutils read (the driver again), and an archive of them.
llvm_cross() {
	disk_apps llvm || return 1
	echo 'int answer(void) { return 42; }' > /tmp/llvm-x.c
	for a in x86_64:X86-64 aarch64:AArch64 riscv64:RISC-V; do
		run-myos -r $DISK_APPS llvm --target=${a%%:*}-unknown-myos -c -o /tmp/llvm-x-${a%%:*}.o /tmp/llvm-x.c || return 1
		run-myos -r $DISK_APPS llvm:llvm-readelf -h /tmp/llvm-x-${a%%:*}.o > /tmp/llvm-x.txt || return 1
		grep -q "Machine:.*${a#*:}" /tmp/llvm-x.txt || { cat /tmp/llvm-x.txt; return 1; }
	done
	rm -f /tmp/llvm-x.a
	run-myos -r $DISK_APPS llvm:llvm-ar rcs /tmp/llvm-x.a /tmp/llvm-x-*.o || return 1
	out=$(run-myos -r $DISK_APPS llvm:llvm-nm /tmp/llvm-x.a | grep -c " T answer")
	echo "$out"
	[ "$out" = 3 ]
}
t -t net,disk llvm_cross llvm_cross

# The linker under its own name, and a shared object against the system's
# libc.so, which a program of the image loads (dl_smoke takes the object
# and the function to call).
llvm_ld() {
	disk_apps llvm || return 1
	run-myos -r $DISK_APPS llvm -c -o /tmp/llvm-hello.o /tmp/llvm-hello.c || return 1
	run-myos -r $DISK_APPS llvm:ld.lld -r -o /tmp/llvm-r.o /tmp/llvm-hello.o || return 1
	run-myos -r $DISK_APPS llvm -o /tmp/llvm-hello2 /tmp/llvm-r.o || return 1
	[ "$(/tmp/llvm-hello2 lld)" = "hello from lld, 3" ] || return 1
	run-myos -r $DISK_APPS llvm -shared -o /tmp/llvm-x.so /tmp/llvm-x.c || return 1
	run-myos -r $DISK_APPS llvm:llvm-readelf -h /tmp/llvm-x.so | grep -q "Type:.*DYN"
}
t -t net,disk,exec,dl llvm_ld llvm_ld
