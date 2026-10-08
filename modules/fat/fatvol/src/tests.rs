//! Host tests against dosfstools and mtools: FAT16 and FAT32 images made by
//! `mkfs.fat`, changed through [`Fat`], must pass `fsck.fat -n`, keep their
//! FAT copies identical, and read back the same through mtools.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::string::{String, ToString};
use std::vec::Vec;
use std::{format, vec};

use fstool::device::SectorDriver;

use crate::{Error, Fat, Kind};

struct FileDev(File, u64);

impl SectorDriver for FileDev {
    type Error = std::io::Error;
    fn sector_size(&self) -> u32 {
        512
    }
    fn sector_count(&self) -> u64 {
        self.1 / 512
    }
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        self.0.read_exact_at(buf, lba * 512)
    }
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error> {
        self.0.write_all_at(buf, lba * 512)
    }
}

/// A fresh 64 MiB image, `mkfs.fat -F bits` (FAT32 with 512-byte clusters,
/// to have the clusters FAT32 needs).
fn image(name: &str, bits: u32) -> PathBuf {
    let dir = std::env::temp_dir().join("fatvol-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}-fat{bits}.img"));
    let f = File::create(&path).unwrap();
    f.set_len(64 << 20).unwrap();
    let mut cmd = Command::new("mkfs.fat");
    cmd.arg("-F").arg(bits.to_string()).arg("-n").arg("TEST");
    if bits == 32 {
        cmd.args(["-s", "1"]);
    }
    let out = cmd.arg(&path).output().expect("mkfs.fat (dosfstools) is needed for these tests");
    assert!(out.status.success(), "mkfs.fat: {}", String::from_utf8_lossy(&out.stderr));
    path
}

fn mount(path: &Path) -> Fat<FileDev> {
    let f = OpenOptions::new().read(true).write(true).open(path).unwrap();
    let len = f.metadata().unwrap().len();
    let mut fs = Fat::mount(FileDev(f, len)).unwrap();
    fs.set_now(1_700_000_000);
    fs
}

/// `fsck.fat -n` finds the image clean, and its FAT copies are identical.
fn check(path: &Path) {
    let out = Command::new("fsck.fat").arg("-n").arg(path).output().expect("fsck.fat (dosfstools)");
    assert!(
        out.status.success(),
        "fsck.fat -n {}:\n{}{}",
        path.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let b = std::fs::read(path).unwrap();
    let u16_at = |o: usize| usize::from(u16::from_le_bytes([b[o], b[o + 1]]));
    let (bps, reserved, n) = (u16_at(11), u16_at(14), usize::from(b[16]));
    let sectors = match u16_at(22) {
        0 => u32::from_le_bytes(b[36..40].try_into().unwrap()) as usize,
        s => s,
    };
    let fat = |i: usize| &b[(reserved + i * sectors) * bps..(reserved + (i + 1) * sectors) * bps];
    assert!((1..n).all(|i| fat(i) == fat(0)), "the FAT copies differ");
}

fn mtools(args: &[&str]) -> Vec<u8> {
    let out = Command::new(args[0])
        .args(&args[1..])
        .env("MTOOLS_SKIP_CHECK", "1")
        .output()
        .expect("mtools is needed for these tests");
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    out.stdout
}

/// The contents of `path` on the image, through mtools.
fn mtype(img: &Path, path: &str) -> Vec<u8> {
    mtools(&["mtype", "-i", img.to_str().unwrap(), &format!("::{path}")])
}

/// The names in the directory `path`, through mtools (`mdir -b`).
fn mls(img: &Path, path: &str) -> Vec<String> {
    let out = mtools(&["mdir", "-b", "-i", img.to_str().unwrap(), &format!("::{path}")]);
    let mut names: Vec<String> = String::from_utf8_lossy(&out)
        .lines()
        .map(|l| l.trim_start_matches("::").trim_start_matches(path).trim_start_matches('/').to_string())
        .map(|n| n.trim_end_matches('/').to_string())
        .filter(|n| !n.is_empty())
        .collect();
    names.sort();
    names
}

fn names(fs: &mut Fat<FileDev>, path: &str) -> Vec<String> {
    let mut out = Vec::new();
    fs.list(path, |n| {
        out.push(String::from_utf8_lossy(n).into_owned());
        true
    })
    .unwrap();
    out.sort();
    out
}

fn read_all(fs: &mut Fat<FileDev>, path: &str) -> Vec<u8> {
    let size = fs.stat(path).unwrap().size as usize;
    let mut out = vec![0u8; size];
    let mut at = 0;
    while at < size {
        let n = fs.read(path, at as u64, &mut out[at..]).unwrap();
        assert!(n > 0, "{path}: short read at {at}");
        at += n;
    }
    out
}

fn put(fs: &mut Fat<FileDev>, path: &str, data: &[u8]) {
    fs.create(path).unwrap();
    fs.truncate(path).unwrap();
    assert_eq!(fs.write(path, 0, data).unwrap(), data.len());
}

/// Deterministic bytes for `seed`.
fn content(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn both(test: impl Fn(u32)) {
    test(16);
    test(32);
}

#[test]
fn reads_what_mtools_wrote() {
    both(|bits| {
        let img = image("mtools", bits);
        let src = std::env::temp_dir().join(format!("fatvol-tests/src{bits}"));
        let _ = std::fs::remove_dir_all(&src);
        std::fs::create_dir_all(src.join("EFI/BOOT")).unwrap();
        std::fs::create_dir_all(src.join("A Long Directory Name/sub")).unwrap();
        std::fs::write(src.join("EFI/BOOT/BOOTX64.EFI"), content(1, 200_000)).unwrap();
        std::fs::write(src.join("A Long Directory Name/sub/Some File Name.data"), content(2, 3000)).unwrap();
        std::fs::write(src.join("MSG"), "fat-msg\n").unwrap();
        let i = img.to_str().unwrap();
        for p in ["EFI", "A Long Directory Name", "MSG"] {
            mtools(&["mcopy", "-s", "-i", i, src.join(p).to_str().unwrap(), "::/"]);
        }
        let mut fs = mount(&img);
        // An 8.3 name alone is listed in lower case; lookups ignore case.
        assert_eq!(names(&mut fs, ""), ["A Long Directory Name", "efi", "msg"]);
        assert_eq!(names(&mut fs, "efi/boot"), ["bootx64.efi"]);
        assert_eq!(read_all(&mut fs, "msg"), b"fat-msg\n");
        assert_eq!(read_all(&mut fs, "EFI/BOOT/BOOTX64.EFI"), content(1, 200_000));
        assert_eq!(read_all(&mut fs, "a long directory name/SUB/some file name.data"), content(2, 3000));
        assert_eq!(fs.stat("A Long Directory Name").unwrap().kind, Kind::Dir);
        assert_eq!(fs.stat("nope").unwrap_err(), Error::NotFound);
        fs.unmount().unwrap();
        check(&img);
    });
}

#[test]
fn edits_stay_consistent() {
    both(|bits| {
        let img = image("edits", bits);
        let mut fs = mount(&img);
        fs.mkdir("boot").unwrap();
        fs.mkdir("boot/slot-a").unwrap();
        assert_eq!(fs.mkdir("BOOT").unwrap_err(), Error::Exists);
        put(&mut fs, "boot/slot-a/kernel", &content(1, 1_200_000));
        put(&mut fs, "boot/slot-a/initramfs", &content(2, 3_000_000));
        put(&mut fs, "boot/A rather long file name.txt", b"long\n");
        // Shorter, then longer content; a write in the middle; a cut.
        put(&mut fs, "boot/slot-a/kernel", &content(3, 700_000));
        put(&mut fs, "boot/slot-a/kernel", &content(4, 1_500_000));
        fs.write("boot/slot-a/kernel", 1000, b"middle").unwrap();
        fs.set_size("boot/slot-a/initramfs", 100_000).unwrap();
        // A directory grown past a cluster, a third of it removed.
        fs.mkdir("many").unwrap();
        for i in 0..300 {
            put(&mut fs, &format!("many/a fairly long entry name {i:04}"), &content(i, (i * 37) as usize));
        }
        for i in (0..300).step_by(3) {
            fs.unlink(&format!("many/a fairly long entry name {i:04}")).unwrap();
        }
        fs.mkdir("gone").unwrap();
        put(&mut fs, "gone/x", b"x");
        assert_eq!(fs.rmdir("gone").unwrap_err(), Error::NotEmpty);
        assert_eq!(fs.unlink("gone").unwrap_err(), Error::IsDir);
        assert_eq!(fs.rmdir("gone/x").unwrap_err(), Error::NotDir);
        fs.unlink("gone/x").unwrap();
        fs.rmdir("gone").unwrap();
        fs.create("boot").unwrap_err();
        fs.unmount().unwrap();
        check(&img);

        let mut kernel = content(4, 1_500_000);
        kernel[1000..1006].copy_from_slice(b"middle");
        assert_eq!(mtype(&img, "/boot/slot-a/kernel"), kernel);
        assert_eq!(mtype(&img, "/boot/slot-a/initramfs"), content(2, 3_000_000)[..100_000]);
        assert_eq!(mtype(&img, "/many/a fairly long entry name 0001"), content(1, 37));
        assert_eq!(mls(&img, "/many").len(), 200);
        let mut fs = mount(&img);
        assert_eq!(read_all(&mut fs, "boot/slot-a/kernel"), kernel);
        assert_eq!(names(&mut fs, "").len(), 2);
    });
}

/// Growing a file reads back as zeros, whatever the clusters it takes held
/// before (fstool's own growing leaves their old bytes in the file).
#[test]
fn growing_writes_zeros() {
    both(|bits| {
        let img = image("zeros", bits);
        let mut fs = mount(&img);
        put(&mut fs, "old", &content(7, 2_000_000));
        fs.unlink("old").unwrap();
        put(&mut fs, "grown", b"0123456789");
        fs.set_size("grown", 1_000_000).unwrap();
        let mut want = vec![0u8; 1_000_000];
        want[..10].copy_from_slice(b"0123456789");
        assert_eq!(read_all(&mut fs, "grown"), want);
        put(&mut fs, "gap", b"abc");
        fs.write("gap", 900_000, b"end").unwrap();
        let mut want = vec![0u8; 900_003];
        want[..3].copy_from_slice(b"abc");
        want[900_000..].copy_from_slice(b"end");
        assert_eq!(read_all(&mut fs, "gap"), want);
        fs.unmount().unwrap();
        check(&img);
    });
}

#[test]
fn rename_files_and_directories() {
    both(|bits| {
        let img = image("rename", bits);
        let mut fs = mount(&img);
        fs.mkdir("a").unwrap();
        fs.mkdir("a/sub").unwrap();
        fs.mkdir("b").unwrap();
        put(&mut fs, "a/f", &content(1, 100_000));
        put(&mut fs, "a/sub/inner", b"inner\n");
        put(&mut fs, "b/old", b"replaced\n");
        // In the same directory, to a long name, then across directories
        // over a file that is there.
        fs.rename("a/f", "a/A much longer name for f").unwrap();
        fs.rename("a/A much longer name for f", "b/old").unwrap();
        assert_eq!(read_all(&mut fs, "b/old"), content(1, 100_000));
        assert_eq!(fs.stat("a/f").unwrap_err(), Error::NotFound);
        // A directory to another parent: its `..` follows (fsck checks it).
        fs.rename("a/sub", "b/moved").unwrap();
        assert_eq!(read_all(&mut fs, "b/moved/inner"), b"inner\n");
        fs.rename("b/moved", "top").unwrap();
        // Only the case changes.
        fs.rename("top", "Top").unwrap();
        assert_eq!(names(&mut fs, ""), ["Top", "a", "b"]);
        // What may not be done.
        fs.mkdir("full").unwrap();
        put(&mut fs, "full/x", b"x");
        assert_eq!(fs.rename("Top", "full").unwrap_err(), Error::NotEmpty);
        assert_eq!(fs.rename("Top", "b/old").unwrap_err(), Error::NotDir);
        assert_eq!(fs.rename("b/old", "full").unwrap_err(), Error::IsDir);
        assert_eq!(fs.rename("b", "b/c").unwrap_err(), Error::Invalid);
        assert_eq!(fs.rename("nope", "x").unwrap_err(), Error::NotFound);
        // A directory over an empty one.
        fs.mkdir("empty").unwrap();
        fs.rename("Top", "empty").unwrap();
        fs.unmount().unwrap();
        check(&img);
        assert_eq!(mtype(&img, "/b/old"), content(1, 100_000));
        assert_eq!(mtype(&img, "/empty/inner"), b"inner\n");
        assert_eq!(mls(&img, "/"), ["a", "b", "empty", "full"]);
    });
}

#[test]
fn set_times() {
    both(|bits| {
        let img = image("times", bits);
        let mut fs = mount(&img);
        put(&mut fs, "f", b"f");
        let t = 981_173_106; // 2001-02-03 04:05:06 UTC
        fs.set_times("f", None, Some(t)).unwrap();
        assert_eq!(fs.stat("f").unwrap().mtime, t);
        fs.set_times("", Some(t), Some(t)).unwrap();
        fs.unmount().unwrap();
        check(&img);
        let out = String::from_utf8(mtools(&["mdir", "-i", img.to_str().unwrap(), "::/f"])).unwrap();
        assert!(out.contains("2001-02-03") && out.contains("4:05"), "mdir: {out}");
    });
}

/// Filling the volume ends in `NoSpace`; what was written is freed again.
#[test]
fn full_volume() {
    both(|bits| {
        let img = image("full", bits);
        let mut fs = mount(&img);
        fs.mkdir("fill").unwrap();
        let mut n = 0;
        loop {
            let path = format!("fill/f{n}");
            fs.create(&path).unwrap();
            // The write that fills the volume is a short one; the next has
            // nothing left to write.
            match fs.write(&path, 0, &content(n, 1 << 20)) {
                Ok(_) => n += 1,
                Err(e) => {
                    assert_eq!(e, Error::NoSpace);
                    break;
                }
            }
        }
        assert!(n > 40, "only {n} MiB fit");
        fs.unmount().unwrap();
        check(&img);
        let mut fs = mount(&img);
        for i in 0..=n {
            fs.unlink(&format!("fill/f{i}")).unwrap();
        }
        fs.rmdir("fill").unwrap();
        put(&mut fs, "after", &content(9, 10 << 20));
        fs.unmount().unwrap();
        check(&img);
    });
}

/// The order an upgrade writes the boot partition in (issue #351): the new
/// slot's files, a sync, then the config rewritten in place.
#[test]
fn upgrade_order() {
    both(|bits| {
        let img = image("upgrade", bits);
        let mut fs = mount(&img);
        put(&mut fs, "limine.conf", b"path: boot():/boot/slot-a/kernel\n");
        fs.mkdir("boot").unwrap();
        fs.mkdir("boot/slot-b").unwrap();
        put(&mut fs, "boot/slot-b/kernel", &content(1, 1_100_000));
        put(&mut fs, "boot/slot-b/initramfs", &content(2, 4_000_000));
        fs.sync().unwrap();
        put(&mut fs, "limine.conf", b"path: boot():/boot/slot-b/kernel\n");
        fs.unmount().unwrap();
        check(&img);
        assert_eq!(mtype(&img, "/limine.conf"), b"path: boot():/boot/slot-b/kernel\n");
        assert_eq!(mtype(&img, "/boot/slot-b/initramfs"), content(2, 4_000_000));
    });
}

/// Reading a file a buffer at a time, as the VFS does: linear in its
/// length, since `Fat` keeps the file open (a fresh handle walks the chain
/// from the start: 2 s here instead of 30 ms). A measurement, not a check:
/// `cargo test -p fatvol --release -- --ignored --nocapture`.
#[test]
#[ignore]
fn read_speed() {
    let img = image("speed", 32);
    let mut fs = mount(&img);
    let data = content(1, 20 << 20);
    put(&mut fs, "big", &data);
    let t = std::time::Instant::now();
    let mut buf = vec![0u8; 8192];
    let mut at = 0u64;
    while at < data.len() as u64 {
        at += fs.read("big", at, &mut buf).unwrap() as u64;
    }
    std::println!("20 MiB in 8 KiB reads, 512-byte clusters: {:?}", t.elapsed());
}

/// `format` makes a FAT32 volume dosfstools finds clean, over whatever the
/// device held, that mounts empty and takes files and directories.
#[test]
fn format_fat32() {
    let dir = std::env::temp_dir().join("fatvol-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("format-fat32.img");
    // Not zeros: the formatter must clear what it relies on.
    std::fs::write(&path, vec![0xA5u8; 64 << 20]).unwrap();
    let f = OpenOptions::new().read(true).write(true).open(&path).unwrap();
    let mut dev = FileDev(f, 64 << 20);
    crate::format(&mut dev, b"MYOS       ", 0x1234_5678).unwrap();
    check(&path);
    let mut fs = Fat::mount(dev).unwrap();
    assert_eq!(fs.kind(), Some(crate::FatKind::Fat32));
    let mut names = Vec::new();
    fs.list("", |n| {
        names.push(String::from_utf8_lossy(n).to_string());
        true
    })
    .unwrap();
    assert!(names.is_empty(), "a fresh volume lists {names:?}");
    fs.mkdir("boot").unwrap();
    fs.create("boot/kernel").unwrap();
    let data: Vec<u8> = (0..300_000u32).map(|i| i as u8).collect();
    assert_eq!(fs.write("boot/kernel", 0, &data).unwrap(), data.len());
    fs.unmount().unwrap();
    check(&path);
    let out = Command::new("mtype").args(["-i"]).arg(&path).arg("::/boot/kernel").output().expect("mtools");
    assert!(out.status.success() && out.stdout == data, "mtype read back something else");
}
