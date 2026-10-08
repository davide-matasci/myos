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

/// Kernel modules loaded at boot, in load order: every driver and
/// filesystem is one of these; the kernel embeds none. Each ships in the
/// initramfs as `/lib/modules/<name>`, and `/lib/modules/boot.list` names
/// them in this order for the kernel's `modules::load_boot_modules`; the
/// boot drive holds only the kernel and the initramfs.
pub const BOOT_MODULES: &[&str] = &[
    "console",
    "hello",
    "pci_enum",
    "acpi",
    "virtio_blk",
    "nvme",
    "xhci",
    "usb_hub",
    "usb_storage",
    "virtio_net",
    "netfs",
    "fat",
    "ext2",
];

/// Modules behind a root Cargo feature: always built and shipped under
/// `/lib/modules/<name>` (so `insmod` can load one at run time), loaded at
/// boot only when the feature is on. `(module, feature)`, in load order.
pub const OPTIONAL_MODULES: &[(&str, &str)] = &[("linux", "linux_compat")];

/// The modules the kernel loads at boot: [`BOOT_MODULES`] plus the
/// optional ones whose feature is active.
pub fn boot_modules() -> Vec<&'static str> {
    BOOT_MODULES
        .iter()
        .copied()
        .chain(
            OPTIONAL_MODULES
                .iter()
                .filter(|(_, feature)| crate::initramfs::feature_enabled(feature))
                .map(|(m, _)| *m),
        )
        .collect()
}

/// Every module the images carry (`/lib/modules`): boot and optional ones.
pub fn all_modules() -> Vec<&'static str> {
    BOOT_MODULES
        .iter()
        .copied()
        .chain(OPTIONAL_MODULES.iter().map(|(m, _)| *m))
        .collect()
}

/// `/lib/modules/boot.list` in the initramfs: [`boot_modules`], one per
/// line, in load order.
pub fn boot_list() -> String {
    boot_modules().iter().map(|m| format!("{m}\n")).collect()
}

/// The Limine config: `head_extra` lines go before the entry (riscv64's
/// `global_dtb`), `kernel_extra` after `path:` (riscv64's `paging_mode`).
/// Limine loads the kernel and the initramfs, nothing else.
pub fn limine_conf(head_extra: &str, kernel_extra: &str) -> String {
    format!(
        "serial: yes\ntimeout: 0\n{head_extra}\n/myos\n    protocol: limine\n    path: boot():/boot/kernel\n{kernel_extra}    module_path: boot():/boot/initramfs\n"
    )
}

/// The kernel as a boot drive carries it: the ELF cut after its last
/// segment, without section headers. Limine loads what the program headers
/// name; the rest is the debug info and the symbols, a dozen MiB that the
/// ELF under `target/` keeps (`addr2line` on a stalled boot's PC,
/// `src/boot_test.rs`). Done here rather than with a strip tool, which the
/// boot jobs (no Rust toolchain) do not have for the other arches.
pub fn boot_kernel(elf: &[u8]) -> Vec<u8> {
    assert!(
        elf.len() >= 64 && elf[..4] == *b"\x7fELF" && elf[4] == 2 && elf[5] == 1,
        "kernel: not a little-endian ELF64"
    );
    let half = |o: usize| u16::from_le_bytes([elf[o], elf[o + 1]]) as usize;
    let word = |o: usize| u64::from_le_bytes(elf[o..o + 8].try_into().unwrap()) as usize;
    let (phoff, phentsize, phnum) = (word(0x20), half(0x36), half(0x38));
    let mut end = phoff + phentsize * phnum;
    for ph in (0..phnum).map(|i| phoff + i * phentsize) {
        // p_offset + p_filesz
        end = end.max(word(ph + 0x08) + word(ph + 0x20));
    }
    let mut out = elf[..end].to_vec();
    // e_shoff, then e_shnum and e_shstrndx: no section headers.
    out[0x28..0x30].fill(0);
    out[0x3c..0x40].fill(0);
    out
}

const SECTOR: usize = 512;
// 64 MiB no longer fits the packed boot images (55 MiB initramfs + kernels +
// limine-bios.sys twice): the FAT16 writer ran out of clusters. 128 MiB keeps
// headroom as the test suite and prebuilts grow.
const IMAGE_BYTES: usize = 128 * 1024 * 1024;
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
    pub fn bootriscv64(&self) -> PathBuf {
        self.dir.join("BOOTRISCV64.EFI")
    }
    pub fn bios_sys(&self) -> PathBuf {
        self.dir.join("limine-bios.sys")
    }
    pub fn bios_cd(&self) -> PathBuf {
        self.dir.join("limine-bios-cd.bin")
    }
    pub fn uefi_cd(&self) -> PathBuf {
        self.dir.join("limine-uefi-cd.bin")
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
    let cc = std::env::var("CC").unwrap_or_else(|_| "clang".to_string());
    let status = Command::new(&cc)
        .args(["-std=c99", "-O2", "-D_FILE_OFFSET_BITS=64", "-o"])
        .arg(&out)
        .arg(&c)
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {cc} for limine host tool: {e}"));
    if !status.success() {
        panic!("failed to compile limine host tool with {cc} (need clang or $CC)");
    }
}

#[derive(Clone)]
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
    initramfs: &[u8],
) {
    write_esp_image_ex(
        dest,
        kernel,
        efi_name,
        efi_bytes,
        bios_sys,
        initramfs,
        &limine_conf("", ""),
        &[],
    );
}

pub fn write_esp_image_ex(
    dest: &Path,
    kernel: &[u8],
    efi_name: &str,
    efi_bytes: &[u8],
    bios_sys: Option<&[u8]>,
    initramfs: &[u8],
    limine_conf: &str,
    extra: &[DiskFile],
) {
    let mut files = vec![
        DiskFile {
            path: format!("EFI/BOOT/{efi_name}"),
            data: efi_bytes.to_vec(),
        },
        DiskFile {
            path: "boot/kernel".into(),
            data: boot_kernel(kernel),
        },
        DiskFile {
            path: "boot/initramfs".into(),
            data: initramfs.to_vec(),
        },
        DiskFile {
            path: "boot/limine/limine.conf".into(),
            data: limine_conf.as_bytes().to_vec(),
        },
        DiskFile {
            path: "EFI/BOOT/limine.conf".into(),
            data: limine_conf.as_bytes().to_vec(),
        },
        DiskFile {
            path: "limine.conf".into(),
            data: limine_conf.as_bytes().to_vec(),
        },
    ];
    files.extend_from_slice(extra);
    if efi_name.contains("RISCV") {
        files.push(DiskFile {
            path: "startup.nsh".into(),
            data: format!("\\EFI\\BOOT\\{efi_name}\r\n").into_bytes(),
        });
    }
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
/// Root file `MSG` contains exactly `fat-msg\n`.
pub fn write_fat_data_image(dest: &Path) {
    let mut part = vec![0u8; FAT_DATA_IMAGE_BYTES];
    format_and_write_fat16(
        &mut part,
        &[DiskFile {
            path: "MSG".into(),
            data: b"fat-msg\n".to_vec(),
        }],
    );
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(dest, &part).unwrap_or_else(|e| panic!("write {}: {e}", dest.display()));
}

/// The boot test's scratch disk (`docs/testing.md`): `total` bytes, sparse
/// (only the GPT and the first partition's FAT are written), with a GPT of
/// two partitions the guest finds as `/dev/<disk>/p1` and `p3` (entry 2 is
/// empty): a 20 MiB FAT16 of ESP type holding `MSG`, and 64 MiB of Linux
/// data, unformatted.
pub fn write_scratch_gpt_image(dest: &Path, total: u64) {
    use std::os::unix::fs::FileExt;
    const ESP_TYPE: [u8; 16] = [
        0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
    ];
    const LINUX_DATA_TYPE: [u8; 16] = [
        0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47, 0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D, 0xE4,
    ];
    const MIB: u64 = 1024 * 1024 / SECTOR as u64;
    let total_lba = total / SECTOR as u64;
    let backup_lba = total_lba - 1;
    let (esp_first, esp_last) = (MIB, 21 * MIB - 1);
    let (data_first, data_last) = (32 * MIB, 96 * MIB - 1);
    let mut entries = [0u8; 128 * 128];
    let uuid = |n: u8| [0x73, 0x63, 0x72, 0x61, 0x74, 0x63, 0x68, 0x40, 0x80, 0, 0, 0, 0, 0, 0, n];
    write_gpt_entry(&mut entries[0..128], &ESP_TYPE, &uuid(1), esp_first, esp_last, 0, "EFI system");
    write_gpt_entry(&mut entries[256..384], &LINUX_DATA_TYPE, &uuid(3), data_first, data_last, 0, "scratch");
    let crc = crc32(&entries);

    let mut head = vec![0u8; 34 * SECTOR];
    write_protective_mbr(&mut head, total_lba);
    head[SECTOR..SECTOR + 92].copy_from_slice(&gpt_header(1, backup_lba, 2, crc, total_lba));
    head[2 * SECTOR..].copy_from_slice(&entries);
    let mut tail = vec![0u8; 33 * SECTOR];
    tail[..entries.len()].copy_from_slice(&entries);
    tail[32 * SECTOR..32 * SECTOR + 92].copy_from_slice(&gpt_header(backup_lba, 1, backup_lba - 32, crc, total_lba));
    let mut esp = vec![0u8; ((esp_last - esp_first + 1) * SECTOR as u64) as usize];
    format_and_write_fat16(
        &mut esp,
        &[DiskFile {
            path: "MSG".into(),
            data: b"scratch-esp\n".to_vec(),
        }],
    );

    let f = fs::File::create(dest).unwrap_or_else(|e| panic!("create {}: {e}", dest.display()));
    f.set_len(total).unwrap_or_else(|e| panic!("size {}: {e}", dest.display()));
    for (at, bytes) in [(0, &head), (esp_first * SECTOR as u64, &esp), ((backup_lba - 32) * SECTOR as u64, &tail)] {
        f.write_all_at(bytes, at).unwrap_or_else(|e| panic!("write {}: {e}", dest.display()));
    }
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

/// Limine BIOS+UEFI hybrid ISO. Recreates `iso_root`, runs xorriso, then
/// `limine bios-install` with no partition number. Needs `xorriso` on PATH.
pub fn write_x86_iso(
    dest: &Path,
    iso_root: &Path,
    kernel: &Path,
    initramfs: &Path,
    limine: &LimineFiles,
) {
    for p in [limine.bios_cd(), limine.uefi_cd()] {
        if !p.is_file() {
            panic!("Limine CD boot file missing: {}", p.display());
        }
    }

    let _ = fs::remove_dir_all(iso_root);
    fs::create_dir_all(iso_root.join("boot/limine"))
        .unwrap_or_else(|e| panic!("create iso boot/limine: {e}"));
    fs::create_dir_all(iso_root.join("EFI/BOOT"))
        .unwrap_or_else(|e| panic!("create iso EFI/BOOT: {e}"));

    let copy = |src: &Path, rel: &str| {
        let dst = iso_root.join(rel);
        fs::copy(src, &dst).unwrap_or_else(|e| {
            panic!("copy {} -> {}: {e}", src.display(), dst.display())
        });
    };
    let elf = fs::read(kernel).unwrap_or_else(|e| panic!("read {}: {e}", kernel.display()));
    fs::write(iso_root.join("boot/kernel"), boot_kernel(&elf))
        .unwrap_or_else(|e| panic!("write iso boot/kernel: {e}"));
    copy(initramfs, "boot/initramfs");
    copy(&limine.bios_sys(), "boot/limine/limine-bios.sys");
    copy(&limine.bios_cd(), "boot/limine/limine-bios-cd.bin");
    copy(&limine.uefi_cd(), "boot/limine/limine-uefi-cd.bin");
    copy(&limine.bootx64(), "EFI/BOOT/BOOTX64.EFI");

    // The image bundles GPL/LGPL/MPL programs: ship the license texts and the
    // third-party notices (incl. the source offer) with it. See
    // THIRD_PARTY_NOTICES.md.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    for rel in ["LICENSE-MIT", "LICENSE-APACHE", "THIRD_PARTY_NOTICES.md"] {
        copy(&manifest.join(rel), rel);
    }
    fs::create_dir_all(iso_root.join("licenses"))
        .unwrap_or_else(|e| panic!("create iso licenses: {e}"));
    let texts = fs::read_dir(manifest.join("licenses"))
        .unwrap_or_else(|e| panic!("read licenses/: {e}"));
    for entry in texts {
        let entry = entry.unwrap_or_else(|e| panic!("read licenses/ entry: {e}"));
        let rel = format!("licenses/{}", entry.file_name().to_string_lossy());
        copy(&entry.path(), &rel);
    }

    let conf = limine_conf("", "");
    let conf = conf.as_bytes();
    for rel in ["boot/limine/limine.conf", "EFI/BOOT/limine.conf", "limine.conf"] {
        let dst = iso_root.join(rel);
        fs::write(&dst, conf)
            .unwrap_or_else(|e| panic!("write {}: {e}", dst.display()));
    }

    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let status = Command::new("xorriso")
        .args([
            "-as",
            "mkisofs",
            "-R",
            "-r",
            "-J",
            "-b",
            "boot/limine/limine-bios-cd.bin",
            "-no-emul-boot",
            "-boot-load-size",
            "4",
            "-boot-info-table",
            "-hfsplus",
            "-apm-block-size",
            "2048",
            "--efi-boot",
            "boot/limine/limine-uefi-cd.bin",
            "-efi-boot-part",
            "--efi-boot-image",
            "--protective-msdos-label",
        ])
        .arg(iso_root)
        .arg("-o")
        .arg(dest)
        .status()
        .expect("failed to spawn xorriso (needed to build the hybrid ISO)");
    if !status.success() {
        panic!("xorriso failed to write {}", dest.display());
    }

    let status = Command::new(limine.tool())
        .arg("bios-install")
        .arg(dest)
        .status()
        .expect("failed to spawn limine bios-install");
    if !status.success() {
        panic!("limine bios-install failed for {}", dest.display());
    }
}

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/limine_gpt.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/limine_fat.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/limine_dir.rs"));
