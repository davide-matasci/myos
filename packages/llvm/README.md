# llvm

LLVM's compiler on myos: clang, lld and the binutils as one program
(`docs/mlibc.md`, "The compiler on myos"). `get-myos llvm`, then
`run-myos llvm hello.c`.

- `fetch.sh`: llvm-project's `llvm`, `clang` and `lld` at
  `toolchain/libcxx`'s pin (one LLVM version in the tree), into
  `target/llvm-src`.
- `llvm-myos.myos.patch`: the `myos` OS of the triple, clang's target and
  driver toolchain for it, and the two places LLVM's support library asks
  what system it is on (the program's path, whether a file is local).
- `build.sh`: CMake's cross build against mlibc and libc++ with the build
  machine's clang, for the arches `MYOS_LLVM_ARCHES` lists; it writes
  `target/llvm-<arch>`. About 2,700 compiles an arch.
- `test.sh`: on the running system, a C and a C++ program compiled, linked
  and run; an object for each of the three arches; the linker and a
  shared object.

Not there: LTO plugins and sanitizer runtimes (compiler-rt is the
builtins only), the static analyzer, clang's tools beyond the driver, and
the development trees of the other two arches (a cross link needs
`--sysroot`).
