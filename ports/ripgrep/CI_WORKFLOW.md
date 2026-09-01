# CI workflow wiring for ripgrep (`/c/rg`)

Applied on `feat/ripgrep-userspace` in `.github/workflows/ci.yml`:

1. **Path filter `c_userspace`** includes ripgrep/pcre2/uutils scripts and patches.
2. **Build job** runs `./ports/ripgrep/build.sh` after uutils, before kernel
   `cargo build`. Missing `target/rg-*` is a hard error in `kernel/build.rs`.
3. **ci-build.tar** packs `target/rg-*`, `target/.myos-ripgrep-version`, and
   pcre2 install prefixes so boot jobs restore prebuilt ELFs (no ripgrep rebuild).
4. **Boot restore** (`scripts/ci-restore-or-build.sh`): if `myos`+`bios.img` exist
   but rg ELFs are missing, build ripgrep **and** `cargo clean -p kernel` for all
   three targets + `cargo build`. Do not exit with a stale image.
5. **rust-cache** (build + boot, `prefix-key: limine-8.3-5`) also stores:
   - stamp files `target/.myos-*-version`
   - built ELFs (`target/sbase-*`, `oksh-*`, `coreutils-*`, `std-*`, `hello-*`,
     `ok-*`, `c-hello-*`, `rg-*`, `uutils-*`)
   - ripgrep/pcre2 trees and `target/newlib-riscv64`
   Nightly stays pinned. Prefix bump drops kernels that embedded an empty `/c/rg`.

Skip-if-fresh: `myos_*_is_current` in `scripts/myos-c-userspace-lib.sh` /
`toolchain/std/lib.sh` (std-hello, c-hello, sbase, oksh, coreutils,
ripgrep, newlib). `build-uutils-myos.sh` still chains ripgrep as a fallback;
the second call is a no-op when stamps+ELFs hit cache.
