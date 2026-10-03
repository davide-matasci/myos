# packages/

Ports CI builds and caches but the images do not carry. A directory here has
the same layout as one under `ports/` (`port.env`, `versions.env`, `fetch.sh`,
`build.sh`, patches): **moving `ports/<name>` here takes it out of the image,
moving it back puts it in**, nothing else changes. See `docs/ports.md`.

Empty for now: the package tarballs and `get-myos`, the tool that installs
them on a running system, come with the next changes.
