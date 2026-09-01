#!/usr/bin/env python3
"""Patch ignore DirEntryRaw::from_path for non-unix/non-windows (myos)."""
from __future__ import annotations

import sys
from pathlib import Path

OLD = """    // Placeholder implementation to allow compiling on non-standard platforms
    // (e.g. wasm32).
    #[cfg(not(any(windows, unix)))]
    fn from_path(
        depth: usize,
        pb: PathBuf,
        link: bool,
    ) -> Result<DirEntryRaw, Error> {
        Err(Error::Io(io::Error::new(
            io::ErrorKind::Other,
            "unsupported platform",
        )))
    }"""

NEW = """    // myos (and other non-unix/non-windows): build DirEntryRaw from metadata.
    #[cfg(not(any(windows, unix)))]
    fn from_path(
        depth: usize,
        pb: PathBuf,
        link: bool,
    ) -> Result<DirEntryRaw, Error> {
        let md = fs::metadata(&pb)
            .map_err(|err| Error::Io(err).with_depth(depth).with_path(&pb))?;
        Ok(DirEntryRaw {
            path: pb,
            ty: md.file_type(),
            follow_link: link,
            depth,
        })
    }"""


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {sys.argv[0]} path/to/ignore/src/walk.rs")
    path = Path(sys.argv[1])
    text = path.read_text()
    if "myos (and other non-unix/non-windows)" in text:
        print(f"ignore from_path already patched: {path}")
        return
    if OLD not in text:
        raise SystemExit(f"ignore from_path stub not found in {path}")
    path.write_text(text.replace(OLD, NEW, 1))
    print(f"patched ignore from_path for myos: {path}")


if __name__ == "__main__":
    main()
