# libedit

[libedit](https://thrysoee.dk/editline/), NetBSD's line editor in its
portable release, as a static `libedit.a` against mlibc, for python's
`readline` module (`packages/python`, `readline.backend` is `editline`):
a build dependency, nothing in the image. Its terminal handling is
ncurses' (the mlibc build of `packages/ncurses`).

`build.sh` runs libedit's own `configure` for each arch with the cross cc
of `myos_write_cross_cc` and builds `src/` into
`target/libedit-mlibc-<arch>/{lib,include}` (`histedit.h`, and GNU
readline's interface in `editline/readline.h`). mlibc has `wcsdup`, which
libedit also carries for the systems that lack it: its copy is dropped
from the archive when configure found mlibc's.
