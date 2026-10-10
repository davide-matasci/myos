//! Host tests against e2fsprogs: images made by `mkfs` and by `mke2fs`,
//! changed through [`Fs`], must pass `e2fsck -fn` and read back the same
//! through `debugfs`.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::string::String;
use std::vec::Vec;
use std::{format, vec};

use crate::{Device, Error, Fs, Kind, mkfs};

struct FileDev(File);

impl Device for FileDev {
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> bool {
        self.0.read_exact_at(buf, offset).is_ok()
    }
    fn write(&mut self, offset: u64, buf: &[u8]) -> bool {
        self.0.write_all_at(buf, offset).is_ok()
    }
    fn now(&mut self) -> u32 {
        1_700_000_000
    }
}

fn image(name: &str, bytes: u64) -> PathBuf {
    let dir = std::env::temp_dir().join("ext2fs-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let f = File::create(&path).unwrap();
    f.set_len(bytes).unwrap();
    path
}

fn open(path: &Path) -> FileDev {
    FileDev(OpenOptions::new().read(true).write(true).open(path).unwrap())
}

fn mke2fs(path: &Path, args: &[&str]) {
    let out = Command::new("mke2fs").args(["-q", "-F", "-t", "ext2"]).args(args).arg(path).output().expect("mke2fs");
    assert!(out.status.success(), "mke2fs: {}", String::from_utf8_lossy(&out.stderr));
}

fn e2fsck_clean(path: &Path) {
    let out = Command::new("e2fsck").args(["-fn"]).arg(path).output().expect("e2fsck");
    assert!(
        out.status.success(),
        "e2fsck {}:\n{}{}",
        path.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn debugfs(path: &Path, cmd: &str) -> Vec<u8> {
    let out = Command::new("debugfs").arg("-R").arg(cmd).arg(path).output().expect("debugfs");
    assert!(out.status.success());
    out.stdout
}

/// Bytes that differ from block to block and with the offset.
fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u32).wrapping_mul(2654435761).to_le_bytes()[1] ^ seed).collect()
}

/// Write `data` in `chunk`-byte calls, as the kernel's 2 KiB file I/O does.
fn write_all(fs: &mut Fs<FileDev>, path: &str, data: &[u8], chunk: usize) {
    fs.create(path).unwrap();
    let mut pos = 0;
    while pos < data.len() {
        let n = fs.write(path, pos as u64, &data[pos..(pos + chunk).min(data.len())]).unwrap();
        assert!(n > 0);
        pos += n;
    }
}

fn read_all(fs: &mut Fs<FileDev>, path: &str) -> Vec<u8> {
    let size = fs.stat(path).unwrap().size as usize;
    let mut out = vec![0u8; size];
    let mut pos = 0;
    while pos < size {
        let n = fs.read(path, pos as u64, &mut out[pos..]).unwrap();
        assert!(n > 0);
        pos += n;
    }
    out
}

fn names(fs: &mut Fs<FileDev>, path: &str) -> Vec<String> {
    let mut v = Vec::new();
    fs.list(path, |n| {
        v.push(String::from_utf8(n.to_vec()).unwrap());
        true
    })
    .unwrap();
    v.sort();
    v
}

/// Everything the module does, on the filesystem in `path`; `big` is the
/// size of the large file (past the double-indirect range at 1 KiB blocks).
fn exercise(path: &Path, big: usize) {
    let mut fs = Fs::mount(open(path)).unwrap();
    fs.mkdir("a").unwrap();
    fs.mkdir("a/b").unwrap();
    assert_eq!(fs.mkdir("a"), Err(Error::Exists));
    write_all(&mut fs, "a/small", b"hello ext2\n", 2048);
    let large = pattern(big, 7);
    write_all(&mut fs, "a/b/large", &large, 2048);
    // Read back before the flush: the blocks still cached come from the
    // cache, the others (evicted) from the disk, in one call.
    let mut back = vec![0u8; large.len() - 1000];
    assert_eq!(fs.read("a/b/large", 1000, &mut back).unwrap(), back.len());
    assert!(back[..] == large[1000..]);
    // A hole: a write past the end.
    fs.create("sparse").unwrap();
    fs.write("sparse", 300_000, b"tail").unwrap();
    fs.symlink("a/small", "short").unwrap();
    let long_target = "x/".repeat(60) + "target";
    fs.symlink(&long_target, "long").unwrap();
    // A directory over several blocks, half of it removed again.
    fs.mkdir("many").unwrap();
    for i in 0..600 {
        write_all(&mut fs, &format!("many/file-with-a-longish-name-{i}"), format!("{i}").as_bytes(), 2048);
    }
    for i in (0..600).step_by(2) {
        fs.unlink(&format!("many/file-with-a-longish-name-{i}")).unwrap();
    }
    // Renames: in place, across directories, over a file, a directory.
    write_all(&mut fs, "r1", b"one", 2048);
    write_all(&mut fs, "r2", b"two", 2048);
    fs.rename("r1", "r1-renamed").unwrap();
    fs.rename("r1-renamed", "a/b/r1").unwrap();
    fs.rename("r2", "a/b/r1").unwrap();
    fs.mkdir("d1").unwrap();
    fs.mkdir("d1/inner").unwrap();
    fs.rename("d1", "a/d1").unwrap();
    assert_eq!(fs.rename("a", "a/b/under"), Err(Error::Invalid));
    fs.mkdir("gone").unwrap();
    fs.rmdir("gone").unwrap();
    assert_eq!(fs.rmdir("a"), Err(Error::NotEmpty));
    write_all(&mut fs, "trunc", &pattern(100_000, 3), 4096);
    fs.truncate("trunc").unwrap();
    write_all(&mut fs, "trunc", b"again", 4096);
    let dev = fs.unmount().unwrap();
    drop(dev);

    // Read back through a fresh mount.
    let mut fs = Fs::mount(open(path)).unwrap();
    assert_eq!(read_all(&mut fs, "a/small"), b"hello ext2\n");
    assert!(read_all(&mut fs, "a/b/large") == large);
    let sparse = read_all(&mut fs, "sparse");
    assert_eq!(sparse.len(), 300_004);
    assert!(sparse[..300_000].iter().all(|&b| b == 0) && &sparse[300_000..] == b"tail");
    let mut buf = [0u8; 256];
    let n = fs.readlink("short", &mut buf).unwrap();
    assert_eq!(&buf[..n], b"a/small");
    let n = fs.readlink("long", &mut buf).unwrap();
    assert_eq!(&buf[..n], long_target.as_bytes());
    assert_eq!(fs.stat("long").unwrap().kind, Kind::Symlink);
    assert_eq!(names(&mut fs, "many").len(), 300);
    assert_eq!(read_all(&mut fs, "a/b/r1"), b"two");
    assert_eq!(fs.stat("r1"), Err(Error::NotFound).map(|()| unreachable!()));
    assert_eq!(names(&mut fs, "a"), ["b", "d1", "small"]);
    assert_eq!(fs.stat("a/d1/inner").unwrap().kind, Kind::Dir);
    assert_eq!(fs.stat("a").unwrap().links, 4); // ., b, d1 and the entry in /
    assert_eq!(read_all(&mut fs, "trunc"), b"again");
    drop(fs.unmount().unwrap());

    e2fsck_clean(path);
    // e2fsprogs sees the same files.
    assert_eq!(debugfs(path, "cat /a/small"), b"hello ext2\n");
    assert!(debugfs(path, "cat /a/b/large") == large);
    assert_eq!(debugfs(path, "cat /a/b/r1"), b"two");
    let ls = String::from_utf8(debugfs(path, "ls -p /a/d1/inner/..")).unwrap();
    assert!(ls.contains("/inner/"), "{ls}");
}

impl PartialEq for crate::Stat {
    fn eq(&self, o: &Self) -> bool {
        (self.kind, self.mode, self.size, self.ino, self.links) == (o.kind, o.mode, o.size, o.ino, o.links)
    }
}

#[test]
fn mkfs_images_pass_e2fsck() {
    for (name, size) in [("mk-8m", 8u64 << 20), ("mk-100m", 100 << 20), ("mk-1g", 1 << 30), ("mk-odd", (37 << 20) + 5000)] {
        let path = image(name, size);
        mkfs(&mut open(&path), size).unwrap();
        e2fsck_clean(&path);
    }
}

#[test]
fn our_mkfs_small() {
    let path = image("ours-small", 120 << 20);
    mkfs(&mut open(&path), 120 << 20).unwrap();
    exercise(&path, 70 << 20);
}

#[test]
fn our_mkfs_large() {
    let path = image("ours-large", 1 << 30);
    mkfs(&mut open(&path), 1 << 30).unwrap();
    exercise(&path, 6 << 20);
}

#[test]
fn mke2fs_1k_128() {
    let path = image("e2-1k", 120 << 20);
    mke2fs(&path, &["-b", "1024", "-I", "128"]);
    exercise(&path, 70 << 20);
}

#[test]
fn mke2fs_4k_256() {
    let path = image("e2-4k", 256 << 20);
    mke2fs(&path, &["-b", "4096"]);
    exercise(&path, 6 << 20);
}

#[test]
fn mke2fs_2k_no_filetype() {
    let path = image("e2-2k", 64 << 20);
    mke2fs(&path, &["-b", "2048", "-O", "^filetype"]);
    exercise(&path, 3 << 20);
}

#[test]
fn refuses_ext4() {
    let path = image("e4", 32 << 20);
    let out = Command::new("mke2fs").args(["-q", "-F", "-t", "ext4"]).arg(&path).output().unwrap();
    assert!(out.status.success());
    assert!(matches!(Fs::mount(open(&path)), Err(Error::Unsupported)));
}

#[test]
fn full_disk_is_no_space() {
    let path = image("full", 2 << 20);
    mkfs(&mut open(&path), 2 << 20).unwrap();
    let mut fs = Fs::mount(open(&path)).unwrap();
    fs.create("big").unwrap();
    let chunk = vec![1u8; 4096];
    let mut pos = 0u64;
    loop {
        match fs.write("big", pos, &chunk) {
            Ok(n) => pos += n as u64,
            Err(e) => {
                assert_eq!(e, Error::NoSpace);
                break;
            }
        }
    }
    assert!(pos > 1 << 20);
    drop(fs.unmount().unwrap());
    e2fsck_clean(&path);
}

#[test]
fn set_times_sticks() {
    let path = image("times", 8 << 20);
    mkfs(&mut open(&path), 8 << 20).unwrap();
    let mut fs = Fs::mount(open(&path)).unwrap();
    fs.create("f").unwrap();
    let st = fs.stat("f").unwrap();
    assert_eq!((st.atime, st.mtime), (1_700_000_000, 1_700_000_000));
    fs.set_times("f", Some(946_684_800), None).unwrap();
    fs.set_times("f", None, Some(978_307_200)).unwrap();
    drop(fs);
    let mut fs = Fs::mount(open(&path)).unwrap();
    let st = fs.stat("f").unwrap();
    assert_eq!((st.atime, st.mtime), (946_684_800, 978_307_200));
    assert!(matches!(fs.set_times("missing", Some(0), Some(0)), Err(Error::NotFound)));
    drop(fs);
    e2fsck_clean(&path);
}

#[test]
fn unlinked_file_kept_until_forgotten() {
    let path = image("kept", 8 << 20);
    mkfs(&mut open(&path), 8 << 20).unwrap();
    let mut fs = Fs::mount(open(&path)).unwrap();
    let data = pattern(70_000, 3);
    fs.create("f").unwrap();
    write_all(&mut fs, "f", &data, 4096);
    let ino = fs.unlink_keep("f").unwrap();
    assert!(matches!(fs.stat("f"), Err(Error::NotFound)));
    // Still readable and writable by its inode, apart from a new "f".
    fs.create("f").unwrap();
    let mut back = vec![0u8; data.len()];
    assert_eq!(fs.read_ino(ino, 0, &mut back).unwrap(), data.len());
    assert_eq!(back, data);
    assert_eq!(fs.write_ino(ino, 70_000, b"more").unwrap(), 4);
    assert_eq!(fs.stat_ino(ino).unwrap().size, 70_004);
    assert_eq!(fs.stat("f").unwrap().size, 0);
    // Forgotten, it is freed: the image is clean.
    fs.forget(ino).unwrap();
    drop(fs.unmount().unwrap());
    e2fsck_clean(&path);
}

#[test]
fn set_size_cuts_and_grows() {
    for (name, args) in [("size1k", &["-b", "1024"][..]), ("size4k", &["-b", "4096"][..])] {
        let path = image(name, 64 << 20);
        mke2fs(&path, args);
        let mut fs = Fs::mount(open(&path)).unwrap();
        // Past the double indirect blocks of a 1 KiB filesystem.
        let data = pattern(8 << 20, 7);
        fs.create("f").unwrap();
        write_all(&mut fs, "f", &data, 65536);
        // Cut mid-block: the rest reads as zeros once it grows back.
        fs.set_size("f", 70_001).unwrap();
        assert_eq!(fs.stat("f").unwrap().size, 70_001);
        fs.set_size("f", 300_000).unwrap();
        let back = read_all(&mut fs, "f");
        assert_eq!(back.len(), 300_000);
        assert_eq!(back[..70_001], data[..70_001]);
        assert!(back[70_001..].iter().all(|&b| b == 0));
        // To nothing, and the same by inode for a kept file.
        fs.set_size("f", 0).unwrap();
        assert_eq!(read_all(&mut fs, "f").len(), 0);
        write_all(&mut fs, "f", &data[..20_000], 4096);
        let ino = fs.unlink_keep("f").unwrap();
        fs.set_size_ino(ino, 5).unwrap();
        assert_eq!(fs.stat_ino(ino).unwrap().size, 5);
        fs.forget(ino).unwrap();
        // Every block given back is free again: e2fsck finds no leak.
        drop(fs.unmount().unwrap());
        e2fsck_clean(&path);
    }
}

#[test]
fn file_id_follows_the_file() {
    let path = image("fileid", 8 << 20);
    mkfs(&mut open(&path), 8 << 20).unwrap();
    let mut fs = Fs::mount(open(&path)).unwrap();
    fs.create("a").unwrap();
    let ino = fs.file_id("a").unwrap();
    // The id is the file's: a rename keeps it, the id reaches the file.
    fs.rename("a", "b").unwrap();
    assert_eq!(fs.file_id("b").unwrap(), ino);
    assert!(matches!(fs.file_id("a"), Err(Error::NotFound)));
    assert_eq!(fs.write_ino(ino, 0, b"data").unwrap(), 4);
    assert_eq!(fs.stat("b").unwrap().size, 4);
    fs.set_times_ino(ino, Some(946_684_800), None).unwrap();
    fs.set_times_ino(ino, None, Some(978_307_200)).unwrap();
    let st = fs.stat("b").unwrap();
    assert_eq!((st.atime, st.mtime), (946_684_800, 978_307_200));
    drop(fs.unmount().unwrap());
    e2fsck_clean(&path);
}

/// The counts `dumpe2fs -h` prints for `field` (`Free blocks`, ...).
fn dumpe2fs(path: &Path, field: &str) -> u32 {
    let out = Command::new("dumpe2fs").arg("-h").arg(path).output().expect("dumpe2fs");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.starts_with(&format!("{field}:"))).expect(field);
    line[field.len() + 1..].trim().parse().unwrap()
}

#[test]
fn usage_follows_the_disk() {
    let path = image("usage", 8 << 20);
    mke2fs(&path, &["-b", "4096"]);
    let mut fs = Fs::mount(open(&path)).unwrap();
    let start = fs.usage();
    assert_eq!(start.block_size, 4096);
    assert_eq!(start.blocks, dumpe2fs(&path, "Block count"));
    assert_eq!(start.free_blocks, dumpe2fs(&path, "Free blocks"));
    assert_eq!(start.inodes, dumpe2fs(&path, "Inode count"));
    assert_eq!(start.free_inodes, dumpe2fs(&path, "Free inodes"));
    // A 1 MiB file takes its 256 blocks (and an indirect one), and an inode.
    write_all(&mut fs, "f", &pattern(1 << 20, 3), 65536);
    let full = fs.usage();
    assert!(start.free_blocks - full.free_blocks >= 256, "{start:?} -> {full:?}");
    assert_eq!(full.free_inodes, start.free_inodes - 1);
    fs.sync().unwrap();
    assert_eq!(full.free_blocks, dumpe2fs(&path, "Free blocks"));
    fs.unlink("f").unwrap();
    assert_eq!(fs.usage(), start);
    drop(fs.unmount().unwrap());
    e2fsck_clean(&path);
}
