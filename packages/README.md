# packages/

Ports CI builds and caches but the images do not carry. A directory here has
the same layout as one under `ports/` (`port.env`, `versions.env`, `fetch.sh`,
`build.sh`, patches): **moving `ports/<name>` here takes it out of the image,
moving it back puts it in**, nothing else changes. See `docs/ports.md`.

`get-myos NAME` installs one on a running system, with the packages it
needs (`PORT_RDEPS` in its descriptor); `cargo run -- packages` builds the
tarballs and the index CI publishes. See `docs/packages.md`.
