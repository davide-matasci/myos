# CI workflow wiring (manual if OAuth lacks `workflow` scope)

GitHub rejects pushes that edit `.github/workflows/*` without the `workflow`
OAuth scope. This branch hooks builds via:

- `scripts/ci-build-kernels.sh` — runs `ports/zlib/build.sh` then `ports/git/build.sh`
- `scripts/ci-restore-or-build.sh` — rebuilds missing git/zlib after artifact restore
- `build.rs` — builds x86_64 git if the ELF is missing before initramfs pack
- Pack alias `target/coreutils-git-<arch>-unknown-none` for existing `coreutils-*` globs

When someone with `workflow` scope can edit workflows, also add (mirroring vim):

## `ci.yml` / `iso.yml`

After vim pull/build/push:

```bash
./scripts/ci-registry.sh pull zlib || true
./scripts/ci-registry.sh pull git || true
# …
./ports/zlib/build.sh
./ports/git/build.sh
# …
./scripts/ci-registry.sh push zlib || true
./scripts/ci-registry.sh push git || true
```

Artifact globs / stamps:

```
target/zlib-*
target/git-*
target/.myos-zlib-version
target/.myos-git-version
```
