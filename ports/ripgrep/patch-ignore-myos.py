#!/usr/bin/env python3
"""Patch ignore DirEntryRaw::from_path and from_entry_os (the parallel
walker's) for non-unix/non-windows (myos)."""
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


OLD_ENTRY = """    // Placeholder implementation to allow compiling on non-standard platforms
    // (e.g. wasm32).
    #[cfg(not(any(windows, unix)))]
    fn from_entry_os(
        depth: usize,
        ent: &fs::DirEntry,
        ty: fs::FileType,
    ) -> Result<DirEntryRaw, Error> {
        Err(Error::Io(io::Error::new(
            io::ErrorKind::Other,
            "unsupported platform",
        )))
    }"""

NEW_ENTRY = """    // myos (and other non-unix/non-windows): the entry's own type.
    #[cfg(not(any(windows, unix)))]
    fn from_entry_os(
        depth: usize,
        ent: &fs::DirEntry,
        ty: fs::FileType,
    ) -> Result<DirEntryRaw, Error> {
        Ok(DirEntryRaw { path: ent.path(), ty, follow_link: false, depth })
    }"""


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {sys.argv[0]} path/to/ignore/src/walk.rs")
    path = Path(sys.argv[1])
    text = path.read_text()
    for name, old, new in (("from_path", OLD, NEW), ("from_entry_os", OLD_ENTRY, NEW_ENTRY)):
        if new in text:
            print(f"ignore {name} already patched: {path}")
        elif old in text:
            text = text.replace(old, new, 1)
            print(f"patched ignore {name} for myos: {path}")
        else:
            raise SystemExit(f"ignore {name} stub not found in {path}")
    path.write_text(text)


if __name__ == "__main__":
    main()
