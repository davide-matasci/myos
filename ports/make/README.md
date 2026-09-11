# GNU Make port for myos

Cross-builds GNU make 4.4.1 (`x86_64`, `aarch64`, `riscv64`) with the
newlib + libgloss/myos userspace. Follows the same pattern as
`ports/vim` (no configure run; a hand-tuned `config.h`).

## Layout

- `versions.env` — pinned tarball version + sha256 (fetched at build).
- `fetch.sh` — download + checksum the official ftp.gnu.org tarball.
- `prepare.sh` — extract into `target/make-myos-build`, copy `config.h`.
- `config.h` — feature set matched to what libgloss/myos implements.
- `myos_compat.h` — declarations newlib headers hide or omit.
- `build.sh` — compile `src/*.c` + `lib/` supplements for all 3 arches.

## Choices

- `HAVE_VFORK` off — plain `fork()` (libgloss has no vfork).
- `MAKE_LOAD` off — no dynamically loadable objects (no dlopen).
- `MAKE_JOBSERVER` off — `-jN` falls back to serial job handling
  (jobserver relies on pipe token semantics + `flock` myos doesn't
  provide).
- `ENABLE_NLS` off; no gettext.
- `remote-stub.c` (no Customs); no Guile; no Windows/VMS/Amiga files.
- `lib/fnmatch.c` + `lib/glob.c`-supplied `glob` come from newlib
  headers; make's bundled gnulib pieces are only linked when newlib
  lacks them (currently none needed beyond `concat-filename`).

## Guest usage

`make` is installed at `/bin/custom/make` (initramfs, `port_make`
Cargo feature). Default `PATH` already covers `/bin/custom`, so
`make -f /tmp/Makefile` works from the shell.
