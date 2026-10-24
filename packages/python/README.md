# python

[CPython](https://www.python.org/) 3.13 as a package (`get-myos python`,
`run-myos python`), built against mlibc (`docs/mlibc.md`): the first port
on the second C library. The app has the interpreter at
`/bin/custom/python` (`python3`, `python3.13`) and the standard library at
`/usr/lib/python3.13`, where the interpreter's built-in prefix (`/usr`,
`sys.prefix`) has it: its search from the executable's directory upwards
stops short of the root, so a library at `/lib` would not be found.
`sys.platform` is `myos`. The library is sources only: the
`.pyc` files would double the app's files and bytes (and the time its
install takes), so the interpreter writes them into the app's directory
on the first import instead; a `python313.zip` would cut both the install
and the first imports (issue #573).

CPython's own build system builds it, cross: `host-myos.myos.patch` teaches
`configure` and `config.sub` the `*-unknown-myos` host (configure accepts a
list of hosts for a cross build), `config.site` answers what a cross build
cannot run, and `build.sh` keeps configure off the host's libraries
(`PKG_CONFIG=false`). A python of the same minor on the host runs the
parts of the build a cross build cannot (the frozen modules, the
library's `.pyc` files); without one, `build.sh` builds the same source
for the host once (`target/python-host`).

Every extension module the standard library has is built in (static:
there is no `dlopen` in a static program), zlib among them, from a zlib
built against mlibc here (the `ports/zlib` build is newlib's). Not there:
`_ssl` and `_hashlib` (LibreSSL is built against newlib; the hashes come
from CPython's own `_sha2`, `_md5`, `_blake2`, `_sha3`), `sqlite3`,
`ctypes` (no libffi), `curses`, `readline`, `bz2` and `lzma`, `tkinter`,
and `pip` (no `ensurepip` without ssl); issue #571. The test suite, IDLE
and turtle are left out of the app.

`test.sh` runs the interpreter and the library through it: a thread, a
subprocess, a socket pair, zlib, json, a hash, floating point and a regexp.
