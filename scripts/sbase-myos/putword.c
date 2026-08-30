/* See LICENSE file for copyright and license details. */
#include <string.h>
#include <unistd.h>

#include "util.h"

void
putword(const char *s)
{
	static int first = 1;

	if (!first)
		writeall(1, " ", 1);

	writeall(1, s, strlen(s));
	first = 0;
}
