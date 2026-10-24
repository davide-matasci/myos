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
built against mlibc here (the `ports/zlib` build is newlib's). The
libraries of the others are ports with a build against mlibc beside
their newlib one (a static library serves only the programs of its own
libc: `errno`'s values, `struct stat` and `FILE` differ): `_ssl` and
`_hashlib` over LibreSSL (`ports/libressl`, `target/libressl-mlibc-<arch>`;
its default CA file is the image's `/lib/cacert.pem`, `cafile.myos.patch`
there), `sqlite3` over SQLite (`packages/sqlite`), `curses` and
`_curses_panel` over ncurses (`packages/ncurses`), `readline` over
libedit (`packages/libedit`, `readline.backend` is `editline`).
LibreSSL has no scrypt, BLAKE2 or SHAKE in its EVP interface:
`hashlib-libressl.myos.patch` leaves `hashlib.scrypt` out and
`config.site` tells configure LibreSSL's hashlib is enough (`blake2b`,
`shake_128` and the others come from CPython's own `_blake2` and `_sha3`).
`pip` is in `site-packages`, unpacked from the wheel CPython bundles
(`ensurepip`'s). Not there: `ctypes` (no libffi), `bz2` and `lzma`,
`dbm`, `tkinter` (issue #571). The test suite, IDLE
and turtle are left out of the app.

`test.sh` runs the interpreter and the library through it: a thread, a
subprocess, a socket pair, zlib, json, a hash, floating point and a regexp;
then the modules over the ports' libraries (the default TLS context and
its CA file, a PBKDF2 through `_hashlib`, an SQLite file and its write
lock, curses and readline imported, `pip --version`), and an HTTPS GET
through `urllib` (tagged `net`).
