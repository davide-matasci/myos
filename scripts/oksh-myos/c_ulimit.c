/* myos: ulimit needs getrlimit/setrlimit; skip rather than vendoring a libc. */
#include "sh.h"

int
c_ulimit(char **wp)
{
	(void)wp;
	bi_errorf("ulimit not supported");
	return 1;
}
