// The boot disk (src/limine_disk.rs), the Limine config, the ISO and the
// FAT16 test disks, plus Limine binary fetch.
//
// Included from `build.rs` (`include!`) and compiled into the host crate.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Limine pin: `ports/limine/versions.env`, which the `limine` port
/// (the tool in myos) builds from too.
const LIMINE_PIN: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/ports/limine/versions.env"));

fn limine_pin(key: &str) -> &'static str {
    LIMINE_PIN
        .lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
        .unwrap_or_else(|| panic!("ports/limine/versions.env has no {key}"))
        .trim()
}

/// The pinned Limine release, `12.6.1`.
pub fn limine_version() -> &'static str {
    limine_pin("LIMINE_VERSION")
}

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

/// The Limine config of a boot disk (`docs/install.md`): an entry per slot
/// of `slots` (`a`, `b`: the kernel and the initramfs in `boot/<slot>/`), the
/// first one the default, the others the fallbacks a short timeout lets one
/// pick at the console. `head_extra` lines go before the entries (riscv64's
/// `global_dtb`), `kernel_extra` after each `path:` (riscv64's
/// `paging_mode`). Limine loads the kernel and the initramfs, nothing else.
pub fn limine_conf(head_extra: &str, kernel_extra: &str, slots: &[&str]) -> String {
    let entries: Vec<(String, String, String)> = slots
        .iter()
        .map(|s| (format!("myos {s}"), format!("boot/{s}"), format!("    cmdline: slot={s}\n")))
        .collect();
    limine_conf_entries(head_extra, kernel_extra, &entries)
}

/// The Limine config with an entry per `(name, directory, extra lines)`;
/// a slot's entry tells the kernel which slot it is (`cmdline: slot=a`,
/// `/proc/cmdline`), which `get-myos --upgrade` reads to write the other.
fn limine_conf_entries(head_extra: &str, kernel_extra: &str, entries: &[(String, String, String)]) -> String {
    let timeout = if entries.len() > 1 { 3 } else { 0 };
    let mut conf = format!("serial: yes\ntimeout: {timeout}\ndefault_entry: 1\n{head_extra}");
    for (name, dir, extra) in entries {
        conf += &format!(
            "\n/{name}\n    protocol: limine\n    path: boot():/{dir}/kernel\n{kernel_extra}    module_path: boot():/{dir}/initramfs\n{extra}"
        );
    }
    conf
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
        let url = format!(
            "https://github.com/limine-bootloader/limine/releases/download/v{}/limine-binary.tar.gz",
            limine_version()
        );
        download(&url, &tar_path);
        verify_sha256(&tar_path, limine_pin("LIMINE_SHA256"));
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

/// The boot disk ([`write_boot_disk`]) with Limine, the kernel and the
/// initramfs in slot `a`, slot `b` empty. `efi_name` is e.g. `BOOTX64.EFI`.
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
        &limine_conf("", "", &["a"]),
        &[],
    );
}

/// [`write_esp_image`] with its Limine config and `extra` files on the ESP.
/// The config is in one place, `boot/limine/limine.conf`: switching slots
/// rewrites that file and nothing else.
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
    let esp = limine_esp_files(efi_name, efi_bytes, bios_sys, limine_conf, extra);
    write_slot_image(dest, kernel, initramfs, esp);
}

/// The boot disk with `esp` (Limine's files, [`limine_esp_files`]) and the
/// kernel, the initramfs and their version in slot `a`, slot `b` empty.
pub fn write_slot_image(dest: &Path, kernel: &[u8], initramfs: &[u8], mut esp: Vec<DiskFile>) {
    esp.extend([
        DiskFile {
            path: "boot/a/kernel".into(),
            data: boot_kernel(kernel),
        },
        DiskFile {
            path: "boot/a/initramfs".into(),
            data: initramfs.to_vec(),
        },
        // The slot's release (`/lib/myos-release` of its initramfs): what
        // `get-myos --upgrade` compares the mirror's with.
        DiskFile {
            path: "boot/a/version".into(),
            data: crate::release::Release::current(Path::new(env!("CARGO_MANIFEST_DIR"))).text().into_bytes(),
        },
    ]);
    write_boot_disk(dest, &esp, &["boot/b"]);
}

/// Limine's files on the ESP, beside the slots: the EFI binary `efi_name`
/// (`BOOTX64.EFI`), x86's `limine-bios.sys`, the config and `extra` (a
/// device tree); riscv64's `startup.nsh`. The release carries the same
/// for `get-myos --install` (`src/packages.rs`).
pub fn limine_esp_files(
    efi_name: &str,
    efi_bytes: &[u8],
    bios_sys: Option<&[u8]>,
    limine_conf: &str,
    extra: &[DiskFile],
) -> Vec<DiskFile> {
    let mut files = vec![
        DiskFile {
            path: format!("EFI/BOOT/{efi_name}"),
            data: efi_bytes.to_vec(),
        },
        DiskFile {
            path: "boot/limine/limine.conf".into(),
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
    }
    files
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
    let total_lba = total / SECTOR as u64;
    let (esp_first, esp_last) = (MIB_LBA, 21 * MIB_LBA - 1);
    let uuid = |n: u8| [0x73, 0x63, 0x72, 0x61, 0x74, 0x63, 0x68, 0x40, 0x80, 0, 0, 0, 0, 0, 0, n];
    let parts = [
        GptPart { entry: 0, type_guid: ESP_TYPE, uuid: uuid(1), first: esp_first, last: esp_last, attrs: 0, name: "EFI system" },
        GptPart {
            entry: 2,
            type_guid: LINUX_DATA_TYPE,
            uuid: uuid(3),
            first: 32 * MIB_LBA,
            last: 96 * MIB_LBA - 1,
            attrs: 0,
            name: "scratch",
        },
    ];
    let mut esp = vec![0u8; ((esp_last - esp_first + 1) * SECTOR as u64) as usize];
    format_and_write_fat16(
        &mut esp,
        &[DiskFile {
            path: "MSG".into(),
            data: b"scratch-esp\n".to_vec(),
        }],
    );

    let f = fs::File::create(dest).unwrap_or_else(|e| die(dest, "create", &e));
    f.set_len(total).unwrap_or_else(|e| die(dest, "size", &e));
    write_gpt(&f, total_lba, &parts).unwrap_or_else(|e| die(dest, "GPT", &e));
    f.write_all_at(&esp, esp_first * SECTOR as u64).unwrap_or_else(|e| die(dest, "write", &e));
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

    let conf = limine_conf_entries("", "", &[("myos".into(), "boot".into(), String::new())]);
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
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/limine_disk.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/limine_fat.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/limine_dir.rs"));
