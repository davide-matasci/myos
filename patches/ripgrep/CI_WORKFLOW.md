# CI workflow wiring (apply to `.github/workflows/ci.yml`)

The OAuth token used for this PR cannot update workflow files (`workflow` scope).
Apply these edits on `feat/ripgrep-userspace` (or merge this note into ci.yml):

1. **Path filter `c_userspace`** — add:
   - `scripts/fetch-ripgrep.sh`, `scripts/fetch-pcre2.sh`, `scripts/prepare-ripgrep-myos.sh`
   - `scripts/build-ripgrep-myos.sh`, `scripts/build-pcre2-myos.sh`
   - `scripts/build-uutils-myos.sh`, `scripts/build-coreutils-myos.sh`
   - `vendor/ripgrep-port/**`, `patches/ripgrep/**`, `patches/coreutils/**`

2. **`Swatinem/rust-cache` `cache-directories`** (build + boot jobs) — add:
   - `target/ripgrep-src`, `target/pcre2-src`
   - `target/pcre2-x86_64`, `target/pcre2-aarch64`, `target/pcre2-riscv64`
   - `target/ripgrep-build-*-unknown-myos`
   - `target/patched-crates`, `target/crate-fetch-coreutils`

3. **Build step** — after `./scripts/build-uutils-myos.sh` run:
   `./scripts/build-ripgrep-myos.sh`

4. **Artifact pack** — include `target/rg-*`, `target/.myos-ripgrep-version`, `target/pcre2-*`

The full updated file is in the PR branch working tree / this agent’s local checkout
as `.github/workflows/ci.yml` (not yet on the remote branch tip).

## Workaround without editing `ci.yml`

`scripts/build-uutils-myos.sh` now invokes `build-ripgrep-myos.sh` at the end, so
existing CI that already runs uutils will produce `target/rg-*` and cache via
whatever dirs rust-cache already restores. Explicit cache-directory entries for
ripgrep/pcre2 (documented above) are still recommended for faster CI.
