/* See LICENSE file for copyright and license details. */
#include <string.h>

#include "util.h"

static long
myos_listdir(char *buf, long len)
{
	long ret;
#if defined(__x86_64__)
	__asm__ volatile("syscall"
	    : "=a"(ret)
	    : "a"(8L), "D"(buf), "S"(len), "d"(0L)
	    : "rcx", "r11", "memory");
#elif defined(__aarch64__)
	register long x8 __asm__("x8") = 8;
	register long x0 __asm__("x0") = (long)(uintptr_t)buf;
	register long x1 __asm__("x1") = len;
	register long x2 __asm__("x2") = 0;
	__asm__ volatile("svc #0" : "+r"(x0) : "r"(x8), "r"(x1), "r"(x2) : "memory");
	ret = x0;
#else
#error unsupported arch
#endif
	return ret;
}

static void
usage(void)
{
	eprintf("usage: %s [-1] [path ...]\n", argv0);
}

static void
listdir_root(int one_per_line, int smoke)
{
	char buf[512];
	long len = myos_listdir(buf, sizeof buf);
	size_t pos = 0;
	int first = 1;

	if (len <= 0) {
		weprintf("opendir .:");
		return;
	}
	while ((size_t)len > pos) {
		size_t start = pos;
		while ((size_t)len > pos && buf[pos] != '\n')
			pos++;
		size_t n = pos - start;
		if ((size_t)len > pos)
			pos++;
		if (n == 0)
			continue;
		if (n >= 1 && buf[start] == '.')
			continue;
		if (!first && !one_per_line)
			writeall(1, " ", 1);
		writeall(1, buf + start, n);
		first = 0;
		if (one_per_line)
			writeall(1, "\n", 1);
	}
	if (!first && !one_per_line)
		writeall(1, "\n", 1);
	if (smoke)
		writeall(1, "sls ok\n", 7);
}

int
main(int argc, char *argv[])
{
	int one = 0;
	int smoke = 0;

	argv0 = *argv;
	if (argv0) {
		argc--;
		argv++;
	}

	while (*argv && (*argv)[0] == '-' && (*argv)[1]) {
		if ((*argv)[1] == '1' && (*argv)[2] == '\0') {
			one = 1;
			argc--;
			argv++;
		} else {
			usage();
		}
	}

	if (argc == 0) {
		smoke = 1;
		listdir_root(one, smoke);
		return 0;
	}

	for (; argc; argc--, argv++) {
		if (strcmp(*argv, ".") == 0 || strcmp(*argv, "/") == 0)
			listdir_root(one, 0);
		else
			weprintf("opendir %s:", *argv);
	}

	return 0;
}
