//! Minimal filetime backend for myos (read timestamps via MetadataExt; sets unsupported).
use crate::FileTime;
use std::fs;
use std::io;
use std::os::myos::fs::MetadataExt;
use std::path::Path;

pub fn set_symlink_file_times(_p: &Path, _atime: FileTime, _mtime: FileTime) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "filetime: set_symlink_file_times unsupported on myos",
    ))
}

pub fn from_last_modification_time(meta: &fs::Metadata) -> FileTime {
    FileTime {
        seconds: meta.mtime(),
        nanos: meta.mtime_nsec() as u32,
    }
}

pub fn from_last_access_time(meta: &fs::Metadata) -> FileTime {
    FileTime {
        seconds: meta.atime(),
        nanos: meta.atime_nsec() as u32,
    }
}

pub fn from_creation_time(_meta: &fs::Metadata) -> Option<FileTime> {
    None
}

pub fn open(path: &Path) -> io::Result<fs::File> {
    fs::File::open(path)
}
