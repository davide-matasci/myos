//! tmpfs: small in-memory writable flat namespace mounted at `/tmp/…`.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::fs::StatInfo;

const MAX_FILES: usize = 32;
const NAME_CAP: usize = 32;
const FILE_CAP: usize = 8192;

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

struct File {
    name: String,
    data: Vec<u8>,
}

static FILES: Mutex<Vec<File>> = Mutex::new(Vec::new());

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && name.len() <= NAME_CAP
}

fn find_index(files: &[File], name: &str) -> Option<usize> {
    files.iter().position(|f| f.name == name)
}

/// RO mounts expect lookup; tmpfs stores mutable bytes so this always returns None.
pub fn lookup(_name: &str) -> Option<&'static [u8]> {
    None
}

pub fn register(_name: &str, _bytes: &'static [u8]) -> bool {
    false
}

pub fn create(name: &str) -> bool {
    if !valid_name(name) {
        return false;
    }
    let mut files = FILES.lock();
    if find_index(&files, name).is_some() {
        return true;
    }
    if files.len() >= MAX_FILES {
        return false;
    }
    files.push(File {
        name: String::from(name),
        data: Vec::new(),
    });
    true
}

pub fn truncate(name: &str) -> bool {
    if !valid_name(name) {
        return false;
    }
    let mut files = FILES.lock();
    let Some(i) = find_index(&files, name) else {
        return false;
    };
    files[i].data.clear();
    true
}

pub fn read(name: &str, pos: usize, out: &mut [u8]) -> usize {
    if !valid_name(name) {
        return 0;
    }
    let files = FILES.lock();
    let Some(i) = find_index(&files, name) else {
        return 0;
    };
    let data = &files[i].data;
    let n = out.len().min(data.len().saturating_sub(pos));
    if n != 0 {
        out[..n].copy_from_slice(&data[pos..pos + n]);
    }
    n
}

pub fn write(name: &str, pos: usize, buf: &[u8]) -> Option<usize> {
    if !valid_name(name) {
        return None;
    }
    let mut files = FILES.lock();
    let Some(i) = find_index(&files, name) else {
        return None;
    };
    let data = &mut files[i].data;
    if pos > data.len() {
        return None;
    }
    let end = pos.checked_add(buf.len())?;
    if end > FILE_CAP {
        return None;
    }
    if end > data.len() {
        data.resize(end, 0);
    }
    data[pos..end].copy_from_slice(buf);
    Some(buf.len())
}

pub fn listdir_at(rel: &str, buf: &mut [u8]) -> usize {
    if !rel.is_empty() && rel != "." {
        return 0;
    }
    let files = FILES.lock();
    let mut n = 0;
    for f in files.iter() {
        let name = f.name.as_bytes();
        let need = name.len() + 1;
        if n + need > buf.len() {
            break;
        }
        buf[n..n + name.len()].copy_from_slice(name);
        n += name.len();
        buf[n] = b'\n';
        n += 1;
    }
    n
}

pub fn stat(name: &str) -> Option<StatInfo> {
    if name.is_empty() || name == "." || name == ".." {
        return Some(StatInfo {
            mode: S_IFDIR | 0o755,
            size: 0,
            ino: 1,
            nlink: 2,
        });
    }
    if !valid_name(name) {
        return None;
    }
    let files = FILES.lock();
    let i = find_index(&files, name)?;
    Some(StatInfo {
        mode: S_IFREG | 0o644,
        size: files[i].data.len() as u32,
        ino: (i as u32) + 2,
        nlink: 1,
    })
}
