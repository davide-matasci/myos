//! Files through the myos std: create, write, append, seek and read back,
//! metadata of an open file, rename, symlinks, directories (created and
//! removed recursively), and the error each refusal reports.
#![no_main]

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::Path;

fn check(ok: bool, what: &str) {
    if !ok {
        println!("[ FAIL ] std fs: {what}");
        std::process::exit(1);
    }
}

fn kind<T>(r: std::io::Result<T>) -> Option<ErrorKind> {
    r.err().map(|e| e.kind())
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    let dir = Path::new("/tmp/std-fs");
    let _ = fs::remove_dir_all(dir);
    check(fs::create_dir_all(dir.join("a/b")).is_ok(), "create_dir_all");
    check(fs::create_dir_all(dir.join("a/b")).is_ok(), "create_dir_all of an existing directory");
    check(kind(fs::create_dir(dir.join("a"))) == Some(ErrorKind::AlreadyExists), "create_dir: AlreadyExists");

    let file = dir.join("a/f");
    check(fs::write(&file, b"hello ").is_ok(), "write");
    let mut f = OpenOptions::new().append(true).open(&file).unwrap();
    check(f.write_all(b"world\n").is_ok(), "append");
    drop(f);
    check(fs::read_to_string(&file).ok().as_deref() == Some("hello world\n"), "read back");

    let mut f = OpenOptions::new().read(true).write(true).open(&file).unwrap();
    check(f.seek(SeekFrom::Start(6)).ok() == Some(6), "seek");
    check(f.write_all(b"WORLD").is_ok(), "write at the offset");
    check(f.seek(SeekFrom::Start(0)).is_ok(), "seek back");
    let mut s = String::new();
    check(f.read_to_string(&mut s).is_ok() && s == "hello WORLD\n", "read after seek");
    check(f.metadata().map(|m| m.len()).ok() == Some(12), "metadata of an open file");
    drop(f);

    check(File::create(&file).is_ok() && fs::metadata(&file).map(|m| m.len()).ok() == Some(0), "create truncates");
    check(kind(File::create_new(&file)) == Some(ErrorKind::AlreadyExists), "create_new: AlreadyExists");

    let moved = dir.join("a/b/g");
    check(fs::rename(&file, &moved).is_ok() && !file.exists() && moved.exists(), "rename");
    let link = dir.join("l");
    check(std::os::myos::fs::symlink("a/b/g", &link).is_ok(), "symlink");
    check(fs::read_link(&link).ok().as_deref() == Some(Path::new("a/b/g")), "read_link");

    let missing = dir.join("missing");
    let err = File::open(&missing).unwrap_err();
    check(err.kind() == ErrorKind::NotFound, "open of a missing file: NotFound");
    check(err.to_string().contains("no such file or directory"), "the error's text");
    check(kind(fs::remove_file(&missing)) == Some(ErrorKind::NotFound), "remove_file: NotFound");
    check(kind(fs::remove_dir(dir.join("a"))) == Some(ErrorKind::DirectoryNotEmpty), "remove_dir: DirectoryNotEmpty");
    check(kind(fs::remove_file(dir.join("a"))) == Some(ErrorKind::IsADirectory), "remove_file: IsADirectory");
    check(kind(File::create("/bin/std/fs")) == Some(ErrorKind::ReadOnlyFilesystem), "create on /bin: ReadOnlyFilesystem");

    check(fs::read_dir(dir).map(|d| d.count()).ok() == Some(2), "read_dir");
    check(fs::remove_dir_all(dir).is_ok() && !dir.exists(), "remove_dir_all");
    println!("[ OK ] std fs");
}
