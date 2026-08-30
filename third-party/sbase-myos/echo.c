/* See LICENSE file for copyright and license details. */
#include <string.h>
#include <unistd.h>

#include "util.h"

int
main(int argc, char *argv[])
{
	int nflag = 0;

	argv0 = *argv, argv0 ? (argc--, argv++) : (void *)0;

	if (*argv && !strcmp(*argv, "-n")) {
		nflag = 1;
		argc--, argv++;
	}

	for (; *argv; argc--, argv++)
		putword(*argv);
	if (!nflag) {
		if (argc == 0 && !argv0)
			writeall(1, "sbase ok\n", 9);
		else
			writeall(1, "\n", 1);
	}

	return 0;
}
