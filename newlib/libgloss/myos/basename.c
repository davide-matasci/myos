/* Copyright 2005 Shaun Jackman — myos libgloss copy of newlib unix/basename.c */

#include <libgen.h>
#include <string.h>

char *
basename(char *path)
{
	char *p;

	if (path == NULL || *path == '\0') {
		return ".";
	}
	p = path + strlen(path) - 1;
	while (*p == '/') {
		if (p == path) {
			return path;
		}
		*p-- = '\0';
	}
	while (p >= path && *p != '/') {
		p--;
	}
	return p + 1;
}
