# awk

The one true awk (Aho, Weinberger and Kernighan's, as its second edition
has it: UTF-8, `--csv`), against newlib. sbase has no awk, and a
`configure` script and the `config.status` it writes run one.

```sh
get-myos awk
run-myos awk '{ n += $1 } END { print n }' file
```

The release is a date (`AWK_VERSION`), fetched from Debian's copy of it
(`versions.env`). Its makefile generates two files, which the build
makes on the build machine: the parser (`bison -d awkgram.y`) and
`proctab.c` (the release's `maketab`, compiled for the build machine).
Nothing is patched.
