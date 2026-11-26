#!/bin/sh
# The source files of a LibreSSL library, from its automake file.
#
#     sources.sh DIR/Makefile.am VARIABLE [CONDITION...]
#
# prints the files VARIABLE collects (`libcrypto_la_SOURCES`, ...), one per
# line, with automake's conditionals (`if NAME`, `if !NAME`, `else`,
# `endif`) resolved for the CONDITIONs given as true and the `include`
# directives of files next to it followed. It stands in for configure,
# which a cross build for newlib cannot run (ports/libressl/build.sh); in
# sh, so that the build runs on myos too (scripts/self-host.sh).
set -eu

# The lines of FILE, an included file's in place of its `include`.
lines() (
	dir=$(dirname "$1")
	while read -r first rest || [ -n "$first" ]; do
		if [ "$first" = include ]; then
			# $(top_srcdir)/Makefile.am.common holds flags, no sources.
			case $rest in
			'$('*) ;;
			*) lines "$dir/$rest" ;;
			esac
		else
			printf '%s\n' "$first $rest"
		fi
	done < "$1"
)

makefile=$1 var=$2
shift 2
true=" $* "

# The open blocks, innermost first: each `active:held`, whether its lines
# count and whether its condition held (`else` flips the latter within the
# enclosing block).
stack=1:1
files=
lines "$makefile" | {
	while read -r first rest; do
		active=${stack%%:*}
		case $first in
		if)
			held=0
			case $rest in
			'!'*) case $true in *" ${rest#!} "*) ;; *) held=1 ;; esac ;;
			*) case $true in *" $rest "*) held=1 ;; esac ;;
			esac
			stack="$((active & held)):$held $stack"
			;;
		else)
			held=${stack%% *}
			held=$((1 - ${held#*:}))
			stack=${stack#* }
			stack="$((${stack%%:*} & held)):$held $stack"
			;;
		endif)
			stack=${stack#* }
			;;
		"$var")
			[ "$active" = 1 ] || continue
			case $rest in
			'+='*) files="$files ${rest#+=}" ;;
			'='*) files=${rest#=} ;;
			esac
			;;
		esac
	done
	[ "$stack" = 1:1 ] || { echo "$makefile: unbalanced conditionals" >&2; exit 1; }
	# shellcheck disable=SC2086
	[ -z "$files" ] || printf '%s\n' $files
}
