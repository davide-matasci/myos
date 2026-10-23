// A program's start, from crt0.S: mlibc's startup (__dlapi_enter, the
// static ld) parses the entry stack, sets up the TLS and the stack guard
// and runs the constructors; main's return goes through exit.
#include <stdint.h>
#include <stdlib.h>

#include <bits/ensure.h>
#include <mlibc/elf/startup.h>

extern "C" void __dlapi_enter(uintptr_t *);

extern char **environ;

extern "C" void __mlibc_entry(int (*main_fn)(int argc, char *argv[], char *env[]), uintptr_t *entry_stack) {
	__dlapi_enter(entry_stack);
	auto result = main_fn(mlibc::entry_stack.argc, mlibc::entry_stack.argv, environ);
	exit(result);
}
