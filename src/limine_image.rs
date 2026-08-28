// GPT disk + FAT16 ESP writer, plus Limine binary fetch.
//
// Included from `build.rs` (`include!`) and compiled into the host crate.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const LIMINE_VERSION: &str = "12.6.1";
pub const LIMINE_TARBALL_URL: &str =
    "https://github.com/limine-bootloader/limine/releases/download/v12.6.1/limine-binary.tar.gz";
pub const LIMINE_TARBALL_SHA256: &str =
    "07d054e6297d8c41bee74ddd30024696e4ad811e7e73be28d98dc0a6168fbfeb";

pub const LIMINE_CONF: &str = "\
serial: yes
timeout: 0

/myos
    protocol: limine
    path: boot():/boot/kernel
    module_path: boot():/boot/hello
    module_path: boot():/boot/ok
";

const SECTOR: usize = 512;
const IMAGE_BYTES: usize = 64 * 1024 * 1024;
const BIOS_BOOT_START_LBA: u64 = 2048;
const BIOS_BOOT_END_LBA: u64 = 4095;
const ESP_START_LBA: u64 = 4096;

/// Raw FAT16 data disk for virtio-blk. 16 MiB is just under the writer's
/// FAT16 minimum (4085 clusters with spc=8); 20 MiB is safely in range.
pub const FAT_DATA_IMAGE_BYTES: usize = 20 * 1024 * 1024;

pub struct LimineFiles {
    pub dir: PathBuf,
}

impl LimineFiles {
    pub fn bootx64(&self) -> PathBuf {
        self.dir.join("BOOTX64.EFI")
    }
    pub fn bootaa64(&self) -> PathBuf {
        self.dir.join("BOOTAA64.EFI")
    }
    pub fn bios_sys(&self) -> PathBuf {
        self.dir.join("limine-bios.sys")
    }
    pub fn tool(&self) -> PathBuf {
        self.dir.join("limine")
    }
}

pub fn fetch_limine(cache_dir: &Path) -> LimineFiles {
    fs::create_dir_all(cache_dir).expect("create limine cache");
    let marker = cache_dir.join("BOOTX64.EFI");
    if !marker.is_file() {
        let tar_path = cache_dir.join("limine-binary.tar.gz");
        download(LIMINE_TARBALL_URL, &tar_path);
        verify_sha256(&tar_path, LIMINE_TARBALL_SHA256);
        let status = Command::new("tar")
            .args(["-xzf"])
            .arg(&tar_path)
            .arg("-C")
            .arg(cache_dir)
            .arg("--strip-components=1")
            .status()
            .expect("failed to spawn tar");
        if !status.success() {
            panic!("failed to unpack Limine binary tarball");
        }
    }
    compile_limine_tool(cache_dir);
    let files = LimineFiles {
        dir: cache_dir.to_path_buf(),
    };
    for p in [files.bootx64(), files.bootaa64(), files.bios_sys()] {
        if !p.is_file() {
            panic!("Limine file missing: {}", p.display());
        }
    }
    files
}

fn download(url: &str, dest: &Path) {
    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(dest)
        .arg(url)
        .status()
        .expect("failed to spawn curl (needed to fetch Limine binaries)");
    if !status.success() {
        panic!("curl failed to download {url}");
    }
}

fn verify_sha256(path: &Path, expected: &str) {
    let out = Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("sha256sum");
    let text = String::from_utf8_lossy(&out.stdout);
    let got = text.split_whitespace().next().unwrap_or("");
    if got != expected {
        panic!(
            "Limine tarball sha256 mismatch: got {got}, expected {expected}"
        );
    }
}

fn compile_limine_tool(dir: &Path) {
    let out = dir.join("limine");
    if out.is_file() {
        return;
    }
    let c = dir.join("limine.c");
    let status = Command::new("cc")
        .args(["-std=c99", "-O2", "-D_FILE_OFFSET_BITS=64", "-o"])
        .arg(&out)
        .arg(&c)
        .status()
        .expect("failed to spawn cc for limine host tool");
    if !status.success() {
        panic!("failed to compile limine host tool (need a C compiler)");
    }
}

pub struct DiskFile {
    pub path: String,
    pub data: Vec<u8>,
}

/// GPT disk with a BIOS boot partition and a FAT16 ESP. `efi_name` is e.g. `BOOTX64.EFI`.
pub fn write_esp_image(
    dest: &Path,
    kernel: &[u8],
    efi_name: &str,
    efi_bytes: &[u8],
    bios_sys: Option<&[u8]>,
    hello: &[u8],
    ok: &[u8],
) {
    let mut files = vec![
        DiskFile {
            path: format!("EFI/BOOT/{efi_name}"),
            data: efi_bytes.to_vec(),
        },
        DiskFile {
            path: "boot/kernel".into(),
            data: kernel.to_vec(),
        },
        DiskFile {
            path: "boot/hello".into(),
            data: hello.to_vec(),
        },
        DiskFile {
            path: "boot/ok".into(),
            data: ok.to_vec(),
        },
        DiskFile {
            path: "boot/limine/limine.conf".into(),
            data: LIMINE_CONF.as_bytes().to_vec(),
        },
        DiskFile {
            path: "EFI/BOOT/limine.conf".into(),
            data: LIMINE_CONF.as_bytes().to_vec(),
        },
        DiskFile {
            path: "limine.conf".into(),
            data: LIMINE_CONF.as_bytes().to_vec(),
        },
    ];
    if let Some(sys) = bios_sys {
        files.push(DiskFile {
            path: "boot/limine/limine-bios.sys".into(),
            data: sys.to_vec(),
        });
        // Also at ESP root. Limine searches root /boot /limine /boot/limine.
        files.push(DiskFile {
            path: "limine-bios.sys".into(),
            data: sys.to_vec(),
        });
    }
    let image = build_gpt_fat16(&files);
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(dest, &image).unwrap_or_else(|e| panic!("write {}: {e}", dest.display()));
}

/// Raw FAT16 volume (no GPT) for the second QEMU virtio-blk disk.
/// Root file `MSG` contains exactly `fat ok\n`.
pub fn write_fat_data_image(dest: &Path) {
    let mut part = vec![0u8; FAT_DATA_IMAGE_BYTES];
    format_and_write_fat16(
        &mut part,
        &[DiskFile {
            path: "MSG".into(),
            data: b"fat ok\n".to_vec(),
        }],
    );
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(dest, &part).unwrap_or_else(|e| panic!("write {}: {e}", dest.display()));
}

pub fn bios_install(limine_tool: &Path, image: &Path) {
    let status = Command::new(limine_tool)
        .arg("bios-install")
        .arg(image)
        .arg("1")
        .status()
        .expect("failed to spawn limine bios-install");
    if !status.success() {
        panic!("limine bios-install failed for {}", image.display());
    }
}

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/limine_gpt.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/limine_fat.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/limine_dir.rs"));
