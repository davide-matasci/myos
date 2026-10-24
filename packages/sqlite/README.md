# sqlite

[SQLite](https://sqlite.org/) as a static `libsqlite3.a` against mlibc,
for python's `sqlite3` module (`packages/python`): a build dependency,
nothing in the image and no app of its own (no `sqlite3` shell).

`build.sh` compiles the amalgamation (`sqlite3.c` of the autoconf
tarball) for the three arches into `target/sqlite-mlibc-<arch>/{lib,include}`:
threadsafe, FTS5 in, no extension loading (no `dlopen` in a static
program). Locking is the unix VFS's, POSIX record locks through `fcntl`.
A newlib build for a newlib program would be one more `build_arch` (the
LibreSSL port builds both).
