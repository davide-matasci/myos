/* Copyright 2005 Shaun Jackman — myos libgloss copy of newlib unix/dirname.c
 * Extended to collapse trailing slashes on the remaining directory so
 * dirname("/foo//bar//") yields "/foo" (POSIX / os-test), not "/foo/". */
#include <libgen.h>
#include <string.h>

char *
dirname(char *path)
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
	if (p < path) {
		return ".";
	}
	if (p == path) {
		return "/";
	}
	*p = '\0';
	/* Collapse any trailing slashes left on the dirname prefix. */
	while (p > path && p[-1] == '/') {
		*--p = '\0';
	}
	if (p == path) {
		/* Was "//…" → keep a single root slash. */
		path[0] = '/';
		path[1] = '\0';
		return path;
	}
	return path;
}
