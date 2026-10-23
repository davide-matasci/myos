//! Host launcher: builds a bootable Limine image and starts QEMU.
//!
//! ```text
//! cargo run                 # x86_64 BIOS, graphical
//! cargo run -- bios
//! cargo run -- uefi         # x86_64 UEFI (fetches OVMF into target/ovmf)
//! cargo run -- aarch64      # QEMU virt + AAVMF, serial + ramfb
//! cargo run -- riscv64      # QEMU virt + RISC-V UEFI, serial + ramfb
//! cargo run -- iso          # write target/myos-x86_64.iso (no QEMU)
//! cargo run -- packages     # write the package tarballs and indexes
//! cargo run -- test --skip heavy      # headless BIOS boot running the tests but the long ones
//! cargo run -- aarch64 test           # every test (also uefi, riscv64)
//! ```

mod boot_test;
mod limine_image;
mod initramfs;
mod packages;
#[allow(dead_code)]
mod ports;
mod wifi_net;
mod wifi_sim;
mod release;

use boot_test::{Board, Mode, Selection};

use limine_image::{
    DiskFile, fetch_limine, limine_conf, limine_esp_files, limine_version,
    write_slot_image,
    write_fat_data_image, write_scratch_gpt_image, write_x86_iso,
};
use ovmf_prebuilt::{Arch, FileType, Prebuilt, Source};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio, exit};
use std::time::Duration;

const AARCH64_TARGET: &str = "aarch64-unknown-none-softfloat";
const RISCV64_TARGET: &str = "riscv64imac-unknown-none-elf";
/// QEMU `-smp` for riscv64 (CI and interactive). Packed `virt.dtb` must use the
/// same count: a single-hart DTB plus OpenSBI BSP hartid=1 →
/// `PANIC: riscv: missing struct riscv_hart for BSP`.
const RISCV_SMP: &str = "2";

/// QEMU `virt` machine options for aarch64 (boot and the DTB dump alike).
/// QEMU `virt` for the aarch64 boots. `gic-version` is `$MYOS_AARCH64_GIC`
/// (2, the default: the memory-mapped GICv2 CPU interface; 3: redistributors
/// and the `ICC_*` system registers; CI boots both). `highmem-*=off` keeps
/// the PCIe ECAM and MMIO windows below 4 GiB, where the 32-bit BARs the
/// kernel assigns reach them, and virtio-mmio transports default to legacy (version 1): the driver is v2.
fn aarch64_machine() -> String {
    let gic = match std::env::var("MYOS_AARCH64_GIC").as_deref() {
        Ok("3") => "3",
        _ => "2",
    };
    // `iommu=smmuv3`: the SMMUv3 in front of the PCIe host, the kernel's
    // `iommu/smmuv3.rs` on every boot (`/dev/pci`'s devices confined). QEMU's
    // unit does stage 1 only unless told (`arm-smmuv3.stage`, with
    // `-global` in [`qemu_aarch64`]): the kernel uses stage 2.
    // The GICv3 boots have no ACPI tables (`acpi=off`): the firmware hands
    // the device tree alone, and the board has the PL061 GPIO controller,
    // whose place an ACPI board's GED takes (the `pl061` module's test).
    let acpi = if gic == "3" { ",acpi=off" } else { "" };
    format!("virt,gic-version={gic},highmem-ecam=off,highmem-mmio=off,iommu=smmuv3{acpi}")
}

/// aarch64: the kernel reads the board from the device tree, and the EDK2
/// (AAVMF) boot path hands Limine none, so the image carries QEMU's own
/// dump of it (`global_dtb`), like riscv64.
fn aarch64_limine_conf() -> String {
    limine_conf("global_dtb: boot():/boot/virt-aarch64.dtb\n", "", &["a"])
}

/// riscv64 needs the packed DTB and Sv39 on top of the common config.
fn riscv_limine_conf() -> String {
    limine_conf(
        "randomise_hhdm_base: no\nglobal_dtb: boot():/boot/virt.dtb\n",
        "    paging_mode: sv39\n",
        &["a"],
    )
}
/// Default qemu64 does not advertise x2APIC (CI #49 panicked, #50 fell back
/// to PIC and hung). Limine leaves PIC IRQs dead, so the kernel timer proof
/// needs the x2APIC MSRs.
const X86_CPU: &str = "qemu64,+x2apic";

/// `-machine` for the x86 boots: `q35` (a PCIe PC with an MCFG, the ECAM
/// path of `kernel/src/arch/x86/pci.rs`) with Intel's IOMMU (`kernel/src/
/// iommu/vtd.rs`: `/dev/pci`'s devices confined, the edu test's DMA
/// outside its buffer refused), or `$MYOS_X86_MACHINE` (`pc`: PCI through
/// port 0xCF8, no MCFG, no IOMMU, QEMU's default machine).
fn x86_machine(cmd: &mut Command) {
    match std::env::var("MYOS_X86_MACHINE").as_deref() {
        Ok(machine) if !machine.is_empty() && machine != "q35" => {
            cmd.arg("-machine").arg(machine);
        }
        _ => {
            cmd.arg("-machine").arg("q35");
            cmd.arg("-device").arg("intel-iommu,intremap=off");
        }
    }
}

fn main() {
    let bios_path = env!("BIOS_PATH");
    let uefi_path = env!("UEFI_PATH");

    let args: Vec<String> = std::env::args().skip(1).map(|s| s.to_lowercase()).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_usage();
        return;
    }

    if args.iter().any(|a| a == "--ci") {
        eprintln!("error: `--ci` is gone; run the boot tests with `test` (docs/testing.md)");
        exit(2);
    }
    let test = Selection::parse(&args).map(Mode::Tests);
    let what = args
        .iter()
        .find(|a| matches!(a.as_str(), "uefi" | "bios" | "aarch64" | "riscv64" | "iso" | "packages" | "rpi4" | "rpi3"))
        .map(|s| s.as_str())
        .unwrap_or("bios");

    match (what, test) {
        ("packages", _) => {
            // The release's boot files take each arch's kernel.
            build_aarch64_kernel();
            build_riscv64_kernel();
            let out = packages::build_all(&PathBuf::from(env!("CARGO_MANIFEST_DIR")));
            println!("{}", out.display());
        }
        ("iso", Some(_)) => {
            eprintln!("error: the ISO is not boot-tested; the bios and uefi images carry the same files");
            exit(2);
        }
        ("iso", None) => run_iso(),
        ("rpi4", Some(_)) => {
            eprintln!("error: the Raspberry Pi 4 image is not boot-tested (no QEMU model runs its firmware)");
            exit(2);
        }
        ("rpi4", None) => println!("{}", build_rpi4_image().display()),
        ("rpi3", Some(mode)) => run_test_rpi3(mode),
        ("rpi3", None) => run_rpi3(),
        ("riscv64", Some(mode)) => run_test_riscv64(mode),
        ("riscv64", None) => run_riscv64(),
        ("aarch64", Some(mode)) => run_test_aarch64(mode),
        ("aarch64", None) => run_aarch64(),
        ("uefi", Some(mode)) => run_test_uefi(uefi_path, mode),
        ("uefi", None) => run_uefi(uefi_path),
        (_, Some(mode)) => run_test_bios(bios_path, mode),
        (_, None) => run_bios(bios_path),
    }
}

fn print_usage() {
    eprintln!(
        "\
Usage: cargo run -- [bios|uefi|aarch64|riscv64] [test [--tags TAG,...] [--skip TAG,...] [--budget MINUTES]]
       cargo run -- iso | packages | rpi4 | rpi3

  bios       Boot the x86_64 Limine BIOS disk image in QEMU (default, graphical)
  uefi       Boot the x86_64 Limine UEFI disk image in QEMU (fetches OVMF on first run)
  aarch64    Boot the AArch64 kernel via Limine on QEMU virt + AAVMF (serial + ramfb)
  riscv64    Boot the RISC-V64 kernel via Limine on QEMU virt + UEFI (serial + ramfb)
  test       Headless: log in, run the tests in the guest, exit with its result. Every test
             carries tags (docs/testing.md), its own name among them: --tags takes those
             with any of the tags, --skip leaves out those with any; `test-mini` is
             `test --skip net,pkg,heavy,install,alpine-disk` (no network, a few minutes), `test-full` is `test`
  iso        Write target/myos-x86_64.iso (Limine BIOS+UEFI hybrid) and exit; needs xorriso
  packages   Write the package tarballs and indexes of the three arches to target/packages/
  rpi4       Write target/rpi4.img (the AArch64 boot disk for a Raspberry Pi 4 behind its
             UEFI firmware, docs/raspberry-pi.md) and exit
  rpi3       Boot a Raspberry Pi 3 in QEMU (raspi3b: U-Boot, Limine, serial on stdio); the
             files for a real one are left in target/rpi3 (docs/raspberry-pi.md)

Environment (the interactive runs, not `test`; see README, A persistent disk):
  MYOS_DISK=PATH       attach PATH as an extra virtio disk, created sparse (8 GiB) if missing
  MYOS_BOOT_DISK=PATH  boot PATH (a disk `get-myos --install` made) instead of the built image",
    );
}

fn run_iso() {
    println!("{}", build_iso().display());
}

/// Write target/myos-x86_64.iso from the built kernel and initramfs (which
/// carries the modules).
fn build_iso() -> PathBuf {
    // Artifact-dep kernel lives at CARGO_BIN_FILE_KERNEL_kernel, not
    // target/<triple>/debug/kernel (ISO #1 panicked on that missing path).
    let kernel = Path::new(env!("KERNEL_PATH"));
    if !kernel.is_file() {
        panic!("kernel ELF missing at {}", kernel.display());
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = manifest.join("target");
    let initramfs_path = target.join("initramfs-x86_64.cpio");
    std::fs::write(&initramfs_path, initramfs::build_initramfs(&manifest, "x86_64", &esp_limine_files("x86_64")))
        .expect("write target/initramfs-x86_64.cpio");
    let limine = fetch_limine(Path::new(env!("LIMINE_DIR")));
    let dest = target.join("myos-x86_64.iso");
    let iso_root = target.join("iso_root");
    write_x86_iso(&dest, &iso_root, kernel, &initramfs_path, &limine);
    dest
}

fn fat_img_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fat.img")
}

fn empty_img_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/empty.img")
}

fn write_empty_blk_image() {
    let dest = empty_img_path();
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&dest, vec![0u8; 64 * 1024])
        .unwrap_or_else(|e| panic!("write {}: {e}", dest.display()));
}

fn scsi_img_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/scsi.img")
}

/// The virtio-scsi test disk: 1 MiB, empty (`kernel.sh`'s `scsi_disk`
/// writes and reads it back).
fn write_scsi_image() {
    let dest = scsi_img_path();
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&dest, vec![0u8; 1024 * 1024]).unwrap_or_else(|e| panic!("write {}: {e}", dest.display()));
}

/// A virtio-scsi host (`device`: the PCI or the virtio-mmio one) with
/// the test disk as its target 0.
fn add_virtio_scsi(cmd: &mut Command, device: &str) {
    write_scsi_image();
    cmd.arg("-device").arg(format!("{device},id=scsi0"));
    cmd.arg("-drive").arg(format!("if=none,id=scsi0d,format=raw,file={}", scsi_img_path().display()));
    cmd.arg("-device").arg("scsi-hd,drive=scsi0d,bus=scsi0.0");
}

fn nvme_img_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/nvme.img")
}

fn write_nvme_blk_image() {
    let dest = nvme_img_path();
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&dest, vec![0u8; 4 * 1024 * 1024])
        .unwrap_or_else(|e| panic!("write {}: {e}", dest.display()));
}

/// The scratch disk: a second NVMe controller (`/dev/nvme1n1/`), sparse
/// (only what is written takes space) and recreated every boot with a GPT
/// for the partition tests (`write_scratch_gpt_image`). Room for a big
/// filesystem on the whole disk (an Alpine root with a toolchain).
const SCRATCH_BYTES: u64 = 4 << 30;

fn scratch_img_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/scratch.img")
}

fn write_scratch_image() {
    write_scratch_gpt_image(&scratch_img_path(), SCRATCH_BYTES);
}

/// The Linux layer tests tagged `alpine-disk` run Alpine's rustc, Python
/// and an X client from roots on a disk the host prepares
/// (`linux-compat/alpine-disks.sh`, kept in `target/`): the third NVMe
/// controller, `/dev/nvme2n1/data`, its writes dropped with the boot.
static ALPINE_DISK: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn prepare_alpine_disk(arch: &str, mode: &Mode) {
    let Mode::Tests(sel) = mode else { return };
    if !sel.needs_alpine_disk() || !linux_compat_enabled() {
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let status = Command::new(root.join("linux-compat/alpine-disks.sh"))
        .arg(arch)
        .status()
        .expect("run linux-compat/alpine-disks.sh");
    assert!(status.success(), "linux-compat/alpine-disks.sh {arch} failed");
    let _ = ALPINE_DISK.set(root.join(format!("target/alpine-{arch}.img")));
}

fn add_nvme(cmd: &mut Command) {
    write_nvme_blk_image();
    write_scratch_image();
    let img = nvme_img_path();
    cmd.arg("-drive").arg(format!(
        "if=none,id=nvme0,format=raw,file={}",
        img.display()
    ));
    cmd.arg("-device").arg("nvme,drive=nvme0,serial=myos");
    cmd.arg("-drive").arg(format!(
        "if=none,id=nvme1,format=raw,file={}",
        scratch_img_path().display()
    ));
    cmd.arg("-device").arg("nvme,drive=nvme1,serial=myos-scratch");
    if let Some(img) = ALPINE_DISK.get() {
        cmd.arg("-drive").arg(format!(
            "if=none,id=nvme2,format=raw,snapshot=on,file={}",
            img.display()
        ));
        cmd.arg("-device").arg("nvme,drive=nvme2,serial=myos-alpine");
    }
}

/// After a boot: an ext2 filesystem the guest left on the scratch disk must
/// be one e2fsprogs finds clean (`e2fsck -fn`; skipped without e2fsprogs).
fn fsck_scratch_disk() -> bool {
    let img = scratch_img_path();
    let mut magic = [0u8; 2];
    let has_ext2 = std::fs::File::open(&img)
        .and_then(|f| {
            use std::os::unix::fs::FileExt;
            f.read_exact_at(&mut magic, 1024 + 56)
        })
        .is_ok()
        && magic == 0xEF53u16.to_le_bytes();
    if !has_ext2 {
        return true;
    }
    match Command::new("e2fsck").arg("-fn").arg(&img).output() {
        Ok(out) if out.status.success() => {
            eprintln!("ci: e2fsck: the scratch disk's ext2 is clean");
            true
        }
        Ok(out) => {
            eprintln!(
                "error: e2fsck -fn {} found problems:\n{}{}",
                img.display(),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            false
        }
        Err(_) => {
            eprintln!("ci: e2fsck not found; the scratch disk is not checked");
            true
        }
    }
}

/// After a boot: the FAT test disk the guest wrote to (`fat_rw`,
/// `user/tests/kernel.sh`) must be one dosfstools finds clean
/// (`fsck.fat -n`; skipped without dosfstools).
fn fsck_fat_disk() -> bool {
    let img = fat_img_path();
    match Command::new("fsck.fat").arg("-n").arg(&img).output() {
        Ok(out) if out.status.success() => {
            eprintln!("ci: fsck.fat: the FAT test disk is clean");
            true
        }
        Ok(out) => {
            eprintln!(
                "error: fsck.fat -n {} found problems:\n{}{}",
                img.display(),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            false
        }
        Err(_) => {
            eprintln!("ci: fsck.fat not found; the FAT test disk is not checked");
            true
        }
    }
}

fn usb_img_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/usb.img")
}

fn usb_hot_img_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/usb-hot.img")
}

/// The USB bus of every boot (docs/usb.md): an xHCI controller, a hub on
/// its first port and a memory stick behind the hub (`/dev/sda/data`: the same
/// FAT volume as `/dev/vda/data`, in its own file since QEMU locks images). A
/// second stick's drive is defined but plugged into root port 2 only by the
/// hot-plug test, through the monitor (`user/tests/host.sh usb-plug`).
fn add_usb(cmd: &mut Command) {
    let fat = fat_img_path();
    for dest in [usb_img_path(), usb_hot_img_path()] {
        std::fs::copy(&fat, &dest).unwrap_or_else(|e| panic!("copy {} to {}: {e}", fat.display(), dest.display()));
    }
    cmd.arg("-device").arg("qemu-xhci,id=xhci");
    cmd.arg("-device").arg("usb-hub,bus=xhci.0,port=1,id=usbhub");
    cmd.arg("-drive")
        .arg(format!("if=none,id=usb0,format=raw,file={}", usb_img_path().display()));
    cmd.arg("-device").arg("usb-storage,bus=xhci.0,port=1.1,drive=usb0,id=usbdisk");
    cmd.arg("-drive")
        .arg(format!("if=none,id=usbhot,format=raw,file={}", usb_hot_img_path().display()));
}

/// The pointer, on the USB bus of [`add_usb`] (`/dev/console/mouse`,
/// docs/tty.md): a `usb-mouse` in the test boots, where the host moves it
/// through the monitor (`mouse_move` is relative, `user/tests/host.sh
/// mouse`); a `usb-tablet` in a window, which follows the host's pointer
/// without grabbing it.
fn add_pointer(cmd: &mut Command, test: bool) {
    let dev = if test { "usb-mouse" } else { "usb-tablet" };
    cmd.arg("-device").arg(format!("{dev},bus=xhci.0,port=3,id=usbpointer"));
}

fn add_virtio_blk_x86(cmd: &mut Command) {
    write_empty_blk_image();
    let fat = fat_img_path();
    let empty = empty_img_path();
    cmd.arg("-drive")
        .arg(format!("if=none,id=vd0,format=raw,file={}", fat.display()));
    // Transitional virtio-blk-pci: the module drives its modern layout.
    cmd.arg("-device").arg("virtio-blk-pci,drive=vd0");
    cmd.arg("-drive").arg(format!(
        "if=none,id=vd1,format=raw,file={}",
        empty.display()
    ));
    cmd.arg("-device").arg("virtio-blk-pci,drive=vd1");
    add_virtio_scsi(cmd, "virtio-scsi-pci");
    add_nvme(cmd);
    add_usb(cmd);
}

fn add_virtio_blk_aarch64(cmd: &mut Command) {
    write_empty_blk_image();
    let fat = fat_img_path();
    let empty = empty_img_path();
    cmd.arg("-drive")
        .arg(format!("if=none,id=vd0,format=raw,file={}", fat.display()));
    cmd.arg("-device").arg("virtio-blk-device,drive=vd0");
    cmd.arg("-drive").arg(format!(
        "if=none,id=vd1,format=raw,file={}",
        empty.display()
    ));
    cmd.arg("-device").arg("virtio-blk-device,drive=vd1");
    add_virtio_scsi(cmd, "virtio-scsi-device");
    add_nvme(cmd);
    add_usb(cmd);
}

fn add_virtio_blk_riscv64(cmd: &mut Command) {
    add_virtio_blk_aarch64(cmd);
}

/// True when local stress (or similar) already has `python3 -m http.server 8765`
/// listening. QEMU `guestfwd=…-tcp:127.0.0.1:8765` aborts at startup with
/// "Connection refused" if nothing is bound.
///
/// The `packages` test installs this build's packages: pack them for `arch`
/// and serve `target/packages/` on the host loopback, which the guest
/// reaches as http://10.0.2.2:8765. A run that skips `net` stays
/// network-free.
fn start_package_mirror(arch: &str, mode: &Mode) {
    let Mode::Tests(sel) = mode else { return };
    if !sel.needs_network() {
        return;
    }
    let dir = packages::build(&PathBuf::from(env!("CARGO_MANIFEST_DIR")), arch);
    if packages::serve_mirror(dir, arch) {
        eprintln!("package mirror: serving target/packages on 127.0.0.1:{}", packages::MIRROR_PORT);
    } else {
        eprintln!("package mirror: port {} busy, not serving", packages::MIRROR_PORT);
    }
}

/// True when something already listens on the host's `port`: a forward to
/// it would abort QEMU at startup, so the launcher leaves it out.
fn host_listening(port: u16) -> bool {
    use std::net::{SocketAddr, TcpStream};
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

/// QEMU's `edu` device, the userspace PCI driver test's (pcidev_smoke,
/// `/dev/pci`). Its DMA reaches all of memory (its default mask is 28
/// bits).
fn add_edu(cmd: &mut Command) {
    cmd.arg("-device").arg("edu,dma_mask=0xffffffffffffffff");
}

/// The simulated Wi-Fi radio (`src/wifi_sim.rs`): a second virtio-net NIC,
/// `/dev/net1`, whose frames go to the access points on a launcher thread.
fn add_wifi_radio(cmd: &mut Command) {
    let Some(sim) = wifi_sim::start() else {
        eprintln!("warning: no simulated Wi-Fi radio (no UDP socket)");
        return;
    };
    cmd.arg("-netdev").arg(sim.netdev_arg());
    cmd.arg("-device").arg("virtio-net-pci,netdev=wifi0");
}

fn add_virtio_net(cmd: &mut Command) {
    // Local boot-stress packs socket_smoke → 10.0.2.100:80 with host http.server
    // on :8765 and starts that server *before* QEMU. Only add guestfwd when the
    // host port is already listening so CI (example.com via user-net, no :8765)
    // does not abort QEMU before the kernel runs.
    // The DNS server moves off QEMU's default 10.0.2.3, the address the
    // resolvers fall back to without a lease: name lookups then show they
    // use the server DHCP gives (`/net/ndb`, issue #320).
    let mut netdev = String::from("user,id=net0,dns=10.0.2.4");
    if host_listening(8765) {
        netdev.push_str(",guestfwd=tcp:10.0.2.100:80-tcp:127.0.0.1:8765");
    }
    // Listen/accept test (boot_test): the guest announces TCP 2323 and the
    // host connects back through slirp hostfwd. Only add when the host
    // port is free so a locally-running listener does not abort QEMU at
    // startup (same guard as guestfwd above).
    if !host_listening(2323) {
        netdev.push_str(",hostfwd=tcp::2323-:2323");
    }
    // SSH to the dropbear the boot script starts on :2222 (docs/ssh.md;
    // the full mode's test connects to it). Same free-port guard.
    if !host_listening(2222) {
        netdev.push_str(",hostfwd=tcp::2222-:2222");
    }
    // The acme package's test: its ACME server on the host validates the
    // http-01 challenge against nginx in the guest on :5002
    // (packages/acme/host.sh). Same free-port guard.
    if !host_listening(5002) {
        netdev.push_str(",hostfwd=tcp::5002-:5002");
    }
    cmd.arg("-netdev").arg(netdev);
    cmd.arg("-device").arg("virtio-net-pci,netdev=net0");
}

fn run_bios(bios_path: &str) {
    let mut cmd = Command::new("qemu-system-x86_64");
    x86_machine(&mut cmd);
    cmd.arg("-cpu")
        .arg(X86_CPU)
        .arg("-m")
        .arg("1024")
        .arg("-smp")
        .arg("4")
        .arg("-serial")
        .arg("stdio")
        .arg("-nic")
        .arg("none")
        .arg("-boot")
        .arg("menu=off");
    add_virtio_blk_x86(&mut cmd);
    add_pointer(&mut cmd, false);
    add_persistent_disk(&mut cmd, "virtio-blk-pci");
    add_boot_disk(&mut cmd, &boot_image(Path::new(bios_path)));
    add_virtio_net(&mut cmd);
    add_edu(&mut cmd);
    add_wifi_radio(&mut cmd);
    let status = cmd.status().expect("failed to start qemu-system-x86_64");
    exit(status.code().unwrap_or(1));
}

fn run_uefi(uefi_path: &str) {
    let (code, vars) = ovmf_files(Arch::X64);
    let mut cmd = Command::new("qemu-system-x86_64");
    x86_machine(&mut cmd);
    cmd.arg("-cpu")
        .arg(X86_CPU)
        .arg("-m")
        .arg("1024")
        .arg("-smp")
        .arg("4")
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=0,file={},readonly=on",
            code.display()
        ))
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=1,file={},snapshot=on",
            vars.display()
        ))
        .arg("-serial")
        .arg("stdio")
        .arg("-nic")
        .arg("none");
    add_virtio_blk_x86(&mut cmd);
    add_pointer(&mut cmd, false);
    add_persistent_disk(&mut cmd, "virtio-blk-pci");
    add_boot_disk(&mut cmd, &boot_image(Path::new(uefi_path)));
    add_virtio_net(&mut cmd);
    add_edu(&mut cmd);
    add_wifi_radio(&mut cmd);
    let status = cmd.status().expect("failed to start qemu-system-x86_64");
    exit(status.code().unwrap_or(1));
}

fn run_aarch64() {
    let image = build_aarch64_image();
    let mut cmd = qemu_aarch64(&boot_image(&image), false);
    add_persistent_disk(&mut cmd, "virtio-blk-device");
    let status = cmd.status().expect("failed to start qemu-system-aarch64");
    exit(status.code().unwrap_or(1));
}

/// The firmware release of rust-osdev's ovmf-prebuilt the boots use (x86_64
/// UEFI, aarch64, riscv64). Pinned, not `Source::LATEST`: OVMF from
/// edk2-stable202605 raises an invalid-opcode exception in the firmware on
/// a `q35` with `-device intel-iommu` and an xHCI controller, before any
/// boot loader runs; edk2-stable202505 and earlier boot it. AGENTS.md's
/// manual download names the same tag and checksum.
const OVMF: Source = Source::EDK2_STABLE202505_R2;

fn ovmf_files(arch: Arch) -> (PathBuf, PathBuf) {
    let prebuilt = Prebuilt::fetch(OVMF, "target/ovmf").expect("failed to fetch OVMF prebuilt");
    (
        prebuilt.get_file(arch, FileType::Code),
        prebuilt.get_file(arch, FileType::Vars),
    )
}

fn aarch64_firmware() -> (PathBuf, PathBuf) {
    if let Ok(prebuilt) = Prebuilt::fetch(OVMF, "target/ovmf") {
        let code = prebuilt.get_file(Arch::Aarch64, FileType::Code);
        let vars = prebuilt.get_file(Arch::Aarch64, FileType::Vars);
        if code.is_file() && vars.is_file() {
            return (code, vars);
        }
    }
    const CANDIDATES: &[(&str, &str)] = &[
        (
            "/usr/share/AAVMF/AAVMF_CODE.fd",
            "/usr/share/AAVMF/AAVMF_VARS.fd",
        ),
        (
            "/usr/share/qemu-efi-aarch64/QEMU_EFI.fd",
            "/usr/share/AAVMF/AAVMF_VARS.fd",
        ),
    ];
    for (c, v) in CANDIDATES {
        if Path::new(c).is_file() {
            return (PathBuf::from(c), PathBuf::from(v));
        }
    }
    panic!(
        "no AArch64 UEFI firmware found. Install qemu-efi-aarch64 (Debian/Ubuntu) \
         or allow ovmf-prebuilt to download into target/ovmf"
    );
}

/// Whether this build carries the optional Linux layer (`--features
/// linux_compat`, baked in by build.rs): its tests join the list.
fn linux_compat_enabled() -> bool {
    initramfs::active_features().iter().any(|f| f == "linux_compat")
}

fn run_test_bios(bios_path: &str, mode: Mode) {
    start_package_mirror("x86_64", &mode);
    prepare_alpine_disk("x86_64", &mode);
    test_boots("bios", Path::new(bios_path), mode, |disk| qemu_test_bios(disk));
}

fn qemu_test_bios(disk: &Path) -> std::process::Child {
    let mut cmd = Command::new("qemu-system-x86_64");
    x86_machine(&mut cmd);
    cmd.arg("-cpu")
        .arg(X86_CPU)
        .arg("-m")
        // 4096: the old curated-os-test OOMs (live≈1M frames after ~300
        // tests) were exited tasks' address spaces never being reclaimed
        // (task::die marked the task Dead before the reclaim, so preemption
        // abandoned it). Live frames now stay flat (~1.4k at the shell).
        .arg("4096")
        .arg("-smp")
        .arg("4")  // ≥2 APs for parallel-fork RR / make -j
        .args({
            // MYOS_TCG_SINGLE=1: single-threaded TCG — far friendlier on a
            // loaded 4-core host where MTTCG vCPU threads starve each other
            // and the boot stalls mid-CI with no serial output.
            if std::env::var("MYOS_TCG_SINGLE").as_deref() == Ok("1") {
                vec!["-accel", "tcg,thread=single"]
            } else {
                vec![]
            }
        })
        .arg("-serial")
        .arg("stdio")
        .arg("-display")
        .arg("none")
        .arg("-monitor")
        .arg(boot_test::monitor_arg())
        .arg("-gdb")
        .arg("tcp::1234")
        .arg("-device")
        .arg("isa-debug-exit,iobase=0xf4,iosize=0x04")
        .arg("-nic")
        .arg("none")
        .arg("-boot")
        .arg("menu=off")
        .arg("-no-reboot");
    add_virtio_blk_x86(&mut cmd);
    add_pointer(&mut cmd, true);
    add_boot_disk(&mut cmd, disk);
    add_virtio_net(&mut cmd);
    add_edu(&mut cmd);
    add_wifi_radio(&mut cmd);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start qemu-system-x86_64")
}

fn run_test_uefi(uefi_path: &str, mode: Mode) {
    start_package_mirror("x86_64", &mode);
    prepare_alpine_disk("x86_64", &mode);
    test_boots("uefi", Path::new(uefi_path), mode, |disk| qemu_test_uefi(disk));
}

fn qemu_test_uefi(disk: &Path) -> std::process::Child {
    let (code, vars) = ovmf_files(Arch::X64);
    let mut cmd = Command::new("qemu-system-x86_64");
    x86_machine(&mut cmd);
    cmd.arg("-cpu")
        .arg(X86_CPU)
        .arg("-m")
        // 4608: UEFI+GOP leaves less usable RAM than BIOS at the same -m.
        // Keep ~512 MiB above BIOS 4096 for GOP / OVMF overhead.
        .arg("4608")
        .arg("-smp")
        .arg({
            // Local TCG-single: smp=1 keeps serial shell typing reliable; CI
            // runners omit MYOS_TCG_SINGLE and keep smp=4 (UEFI cat|cat race).
            if std::env::var("MYOS_TCG_SINGLE").as_deref() == Ok("1") {
                "1"
            } else {
                "4" // ≥2 APs for parallel-fork RR / make -j
            }
        })
        .args({
            // Match run_ci_bios: local-ci.sh exports MYOS_TCG_SINGLE=1 so MTTCG
            // does not starve the boot. Without this, UEFI CI ignored the flag.
            if std::env::var("MYOS_TCG_SINGLE").as_deref() == Ok("1") {
                vec!["-accel", "tcg,thread=single"]
            } else {
                vec![]
            }
        })
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=0,file={},readonly=on",
            code.display()
        ))
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=1,file={},snapshot=on",
            vars.display()
        ))
        .arg("-serial")
        .arg("stdio")
        .arg("-display")
        .arg("none")
        .arg("-monitor")
        .arg(boot_test::monitor_arg())
        .arg("-device")
        .arg("isa-debug-exit,iobase=0xf4,iosize=0x04")
        .arg("-nic")
        .arg("none")
        .arg("-no-reboot");
    add_virtio_blk_x86(&mut cmd);
    add_pointer(&mut cmd, true);
    add_boot_disk(&mut cmd, disk);
    add_virtio_net(&mut cmd);
    add_edu(&mut cmd);
    add_wifi_radio(&mut cmd);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start qemu-system-x86_64")
}

fn run_test_aarch64(mode: Mode) {
    // The image first: its kernel build makes the user programs the
    // packages' boot files (the initramfs) take.
    let image = build_aarch64_image();
    start_package_mirror("aarch64", &mode);
    prepare_alpine_disk("aarch64", &mode);
    test_boots("aarch64", &image, mode, |disk| {
        qemu_aarch64(disk, true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start qemu-system-aarch64")
    });
}

/// The boot test of the boot disk `image`, on a copy of it
/// (`target/boot-test-<name>.img`): the `install` tests' `get-myos
/// --upgrade` writes its other slot, and `get-myos --install` makes a boot
/// disk of the scratch disk. When the tests passed and those ran, the copy
/// boots again ([`Mode::Reboot`]), which must come up from that slot, and
/// then the installed disk ([`Mode::Installed`], kept as
/// `target/installed-<name>.img`: every boot makes a new scratch disk) as
/// the boot disk, by BIOS on the bios job and UEFI elsewhere. The copy keeps
/// the build's image as it was.
fn test_boots(name: &str, image: &Path, mode: Mode, spawn: impl Fn(&Path) -> std::process::Child) -> ! {
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let disk = target.join(format!("boot-test-{name}.img"));
    limine_image::copy_sparse(image, &disk);
    let linux = linux_compat_enabled();
    let slow = name.starts_with("riscv64");
    let outcome = boot_test::run(spawn(&disk), Board::Virt, &mode, linux, slow);
    let mut ok = outcome.ok;
    let ran = |test: &str| outcome.passed.iter().any(|t| t == test);
    // The installed disk is kept before the next boot makes a new scratch disk.
    let installed = target.join(format!("installed-{name}.img"));
    if ok && ran("boot_install_local") {
        limine_image::copy_sparse(&scratch_img_path(), &installed);
    }
    if ok && ran("boot_upgrade") {
        eprintln!("boot test: the upgraded disk boots again");
        ok = boot_test::run(spawn(&disk), Board::Virt, &Mode::Reboot { app: ran("apps_data") }, linux, slow).ok;
    }
    if ok && ran("boot_install_local") {
        eprintln!("boot test: the disk get-myos --install made boots");
        ok = boot_test::run(spawn(&installed), Board::Virt, &Mode::Installed, linux, slow).ok;
    }
    exit(if ok { 0 } else { 1 })
}

/// x86: the boot disk as a third virtio-blk (`/dev/vdc/`, after
/// `add_virtio_blk_x86`'s two, so the test disks keep their names), the one
/// the firmware boots: myos sees it (`get-myos --upgrade` writes it), which
/// an IDE disk it is not. Not NVMe: with 4 GiB of RAM SeaBIOS maps the NVMe
/// BAR (64-bit) above 4 GiB, out of its reach, and finds no disk to boot;
/// the transitional virtio-blk keeps its legacy BAR0 in I/O ports for the
/// firmware, and the modern layout the module drives beside it.
/// A persistent disk the interactive runs attach with `MYOS_DISK=PATH`
/// (README "A persistent disk"): created sparse on first use, left alone
/// after, so what the guest wrote to it is there on the next run.
const PERSISTENT_DISK_BYTES: u64 = 8 << 30;

fn add_persistent_disk(cmd: &mut Command, device: &str) {
    let Some(path) = std::env::var_os("MYOS_DISK").filter(|p| !p.is_empty()).map(PathBuf::from) else {
        return;
    };
    if !path.exists() {
        std::fs::File::create(&path)
            .and_then(|f| f.set_len(PERSISTENT_DISK_BYTES))
            .unwrap_or_else(|e| panic!("create {}: {e}", path.display()));
        eprintln!("MYOS_DISK: created {} ({} GiB, sparse)", path.display(), PERSISTENT_DISK_BYTES >> 30);
    }
    cmd.arg("-drive")
        .arg(format!("if=none,id=persist,format=raw,file={}", path.display()));
    cmd.arg("-device").arg(format!("{device},drive=persist"));
}

/// The disk an interactive run boots: `MYOS_BOOT_DISK=PATH` (a disk
/// `get-myos --install` made, docs/install.md) instead of the image just
/// built.
fn boot_image(built: &Path) -> PathBuf {
    match std::env::var_os("MYOS_BOOT_DISK").filter(|p| !p.is_empty()) {
        Some(path) => {
            let path = PathBuf::from(path);
            assert!(path.is_file(), "MYOS_BOOT_DISK: {} does not exist", path.display());
            path
        }
        None => built.to_path_buf(),
    }
}

fn add_boot_disk(cmd: &mut Command, image: &Path) {
    cmd.arg("-drive")
        .arg(format!("if=none,id=bootdisk,format=raw,file={}", image.display()));
    cmd.arg("-device").arg("virtio-blk-pci,drive=bootdisk,bootindex=0");
}

fn qemu_aarch64(image: &Path, ci: bool) -> Command {
    let (code, vars) = aarch64_firmware();
    let mut cmd = Command::new("qemu-system-aarch64");
    cmd.arg("-global")
        .arg("virtio-mmio.force-legacy=false")
        .arg("-global")
        .arg("arm-smmuv3.stage=2")
        .arg("-machine")
        .arg(aarch64_machine())
        .arg("-cpu")
        .arg("cortex-a72")
        .arg("-m")
        .arg(if ci { "4096" } else { "1024" })
        .arg("-smp")
        .arg("4")
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=0,file={},readonly=on",
            code.display()
        ))
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=1,file={},snapshot=on",
            vars.display()
        ));
    // Data disks first so they become /dev/vda/ and /dev/vdb/; ESP boots via bootindex.
    add_virtio_blk_aarch64(&mut cmd);
    add_virtio_net(&mut cmd);
    add_edu(&mut cmd);
    add_wifi_radio(&mut cmd);
    cmd.arg("-drive")
        .arg(format!(
            "if=none,id=hd0,format=raw,file={}",
            image.display()
        ))
        .arg("-device")
        .arg("virtio-blk-device,drive=hd0,bootindex=1")
        .arg("-serial")
        .arg("stdio")
        .arg("-nic")
        .arg("none")
        .arg("-no-reboot");
    // The screen in every boot, the headless CI ones too: it is `/dev/fb`
    // (docs/fb.md), which the boot tests draw on.
    cmd.arg("-device").arg("ramfb");
    // The keyboard in every boot too: the boot tests type on it through
    // the monitor (`/dev/console/kbd`, docs/tty.md).
    cmd.arg("-device").arg("virtio-keyboard-device");
    add_pointer(&mut cmd, ci);
    // And a virtio mouse in the test boots, which `user/c/test.sh
    // virtio_mouse` moves (`host.sh mouse virtio`).
    if ci {
        cmd.arg("-device").arg("virtio-mouse-device");
    }
    if ci {
        cmd.arg("-display").arg("none");
        cmd.arg("-monitor").arg(boot_test::monitor_arg());
    }
    cmd
}

fn build_aarch64_image() -> PathBuf {
    let kernel = build_aarch64_kernel();
    let kernel_bytes = std::fs::read(&kernel).expect("read aarch64 kernel ELF");
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/aarch64.img");
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let esp = esp_limine_files("aarch64");
    let initramfs = initramfs::build_initramfs(&manifest, "aarch64", &esp);
    write_slot_image(&image, &kernel_bytes, &initramfs, esp);
    write_fat_data_image(&fat_img_path());
    image
}

/// The AArch64 boot disk for a Raspberry Pi 4 behind its UEFI firmware
/// (`docs/raspberry-pi.md`): the same kernel and initramfs as
/// `target/aarch64.img`, but without QEMU's device tree, so that Limine
/// hands on the firmware's, and with `platform=dt`, so that the kernel
/// reads the board from it even where the firmware has ACPI tables too.
fn build_rpi4_image() -> PathBuf {
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/rpi4.img");
    build_rpi_image(&image);
    image
}

/// The AArch64 boot disk for a Raspberry Pi, at `image`: the kernel and
/// initramfs of `target/aarch64.img`, without QEMU's device tree, so that
/// Limine hands on the firmware's, and with `platform=dt`, so that the
/// kernel reads the board from it even where the firmware has ACPI tables
/// too.
fn build_rpi_image(image: &Path) {
    let kernel = build_aarch64_kernel();
    let kernel_bytes = std::fs::read(&kernel).expect("read aarch64 kernel ELF");
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let limine = limine_files();
    let read = |p: PathBuf| std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    let conf = limine_image::limine_conf_cmdline("", "", &["a"], " platform=dt");
    let esp = limine_esp_files("BOOTAA64.EFI", &read(limine.bootaa64()), None, &conf, &[]);
    let initramfs = initramfs::build_initramfs(&manifest, "aarch64", &esp);
    write_slot_image(image, &kernel_bytes, &initramfs, esp);
}

/// A Raspberry Pi 3 in QEMU (`raspi3b`, docs/raspberry-pi.md): every core
/// starts at 0 in myos's armstub (`boot/rpi3`), which drops to EL1 and
/// starts U-Boot; U-Boot starts Limine from the SD card, which holds the
/// Raspberry Pi boot disk. Memory is laid out as the Pi's firmware leaves
/// it: the device tree at 0x2000000, with its memory node filled in, and
/// its address and U-Boot's in the stub. The kernel's console is the mini
/// UART (the tree's `stdout-path`), on stdio; U-Boot's, the PL011, goes to
/// `target/rpi3/u-boot.log`. The boot files for a real Pi 3 stay in
/// `target/rpi3`.
fn run_rpi3() {
    let status = qemu_rpi3(false).status().expect("failed to start qemu-system-aarch64");
    exit(status.code().unwrap_or(1));
}

/// The boot test on the Raspberry Pi 3 ([`run_rpi3`]): the selection's
/// tests the board can run (it has no disk but its SD card, no network, no
/// PCI or USB: `docs/testing.md`).
fn run_test_rpi3(mode: Mode) -> ! {
    let child = qemu_rpi3(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start qemu-system-aarch64");
    let ok = boot_test::run(child, Board::Rpi3, &mode, linux_compat_enabled(), true).ok;
    exit(if ok { 0 } else { 1 })
}

/// The Raspberry Pi 3's boot files (`boot/rpi3/build.sh`, the boot disk,
/// the firmware's part done) and the QEMU command that boots them: the
/// console on stdio, a monitor for the boot test (`test`).
fn qemu_rpi3(test: bool) -> Command {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out = manifest.join("target/rpi3");
    let status = Command::new(manifest.join("boot/rpi3/build.sh")).status().expect("run boot/rpi3/build.sh");
    if !status.success() {
        eprintln!("error: boot/rpi3/build.sh failed");
        exit(1);
    }
    // The SD card: QEMU wants a power of two.
    let disk = out.join("myos.img");
    build_rpi_image(&disk);
    let len = std::fs::metadata(&disk).expect("rpi3 disk").len();
    let f = std::fs::OpenOptions::new().write(true).open(&disk).expect("open rpi3 disk");
    f.set_len(len.next_power_of_two()).expect("pad rpi3 disk");

    const DTB_AT: u32 = 0x0200_0000;
    const UBOOT_AT: u32 = 0x8_0000;
    let dtb = out.join("qemu.dtb");
    std::fs::copy(out.join("bcm2710-rpi-3-b.dtb"), &dtb).expect("copy the Pi 3 device tree");
    // 960 MiB of the 1 GiB for the ARM (the rest is the GPU's), as the
    // firmware writes it.
    let status = Command::new("fdtput")
        .args(["-t", "x"])
        .arg(&dtb)
        .args(["/memory@0", "reg", "0", "3c000000"])
        .status()
        .expect("run fdtput (device-tree-compiler)");
    if !status.success() {
        eprintln!("error: fdtput could not fill the memory node of {}", dtb.display());
        exit(1);
    }
    let mut stub = std::fs::read(out.join("armstub8-myos.bin")).expect("read the armstub");
    stub[0xf8..0xfc].copy_from_slice(&DTB_AT.to_le_bytes());
    stub[0xfc..0x100].copy_from_slice(&UBOOT_AT.to_le_bytes());
    let stub_path = out.join("armstub8-qemu.bin");
    std::fs::write(&stub_path, stub).expect("write the armstub for QEMU");

    let loader = |file: &Path, at: u32| format!("loader,file={},addr={at:#x},force-raw=on", file.display());
    let mut cmd = Command::new("qemu-system-aarch64");
    // The screen (the firmware's framebuffer, `/dev/fb`) where there is a
    // desktop to show it on.
    if test || (std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none()) {
        cmd.args(["-display", "none"]);
    }
    cmd.args(["-M", "raspi3b"])
        .args(["-device", &loader(&stub_path, 0)])
        .args(["-device", &loader(&out.join("u-boot.bin"), UBOOT_AT)])
        .args(["-device", &loader(&dtb, DTB_AT)])
        .arg("-drive")
        .arg(format!("if=sd,format=raw,file={}", disk.display()))
        .arg("-serial")
        .arg(format!("file:{}", out.join("u-boot.log").display()));
    if test {
        cmd.args(["-serial", "stdio"]).arg("-monitor").arg(boot_test::monitor_arg());
    } else {
        cmd.args(["-serial", "mon:stdio"]);
    }
    cmd
}

/// The Limine release's files (`target/limine-v<version>`, which build.rs
/// fetched).
fn limine_files() -> limine_image::LimineFiles {
    let limine_dir = PathBuf::from(env!("LIMINE_DIR"));
    if limine_dir.join("BOOTAA64.EFI").is_file() {
        return fetch_limine(&limine_dir);
    }
    let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("limine-v{}", limine_version()));
    fetch_limine(&fallback)
}

/// Limine's files on `arch`'s boot disk ESP, beside the slots: the boot
/// images and the release's `<arch>-boot.txt` (`get-myos --install`,
/// `src/packages.rs`) take the same.
pub(crate) fn esp_limine_files(arch: &str) -> Vec<DiskFile> {
    let limine = limine_files();
    let read = |p: PathBuf| std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    match arch {
        "x86_64" => limine_esp_files(
            "BOOTX64.EFI",
            &read(limine.bootx64()),
            Some(&read(limine.bios_sys())),
            &limine_conf("", "", &["a"]),
            &[],
        ),
        "aarch64" => limine_esp_files(
            "BOOTAA64.EFI",
            &read(limine.bootaa64()),
            None,
            &aarch64_limine_conf(),
            &[DiskFile { path: "boot/virt-aarch64.dtb".into(), data: aarch64_dtb() }],
        ),
        "riscv64" => limine_esp_files(
            "BOOTRISCV64.EFI",
            &read(limine.bootriscv64()),
            None,
            &riscv_limine_conf(),
            &[DiskFile { path: "boot/virt.dtb".into(), data: riscv64_dtb() }],
        ),
        _ => panic!("no boot disk for {arch}"),
    }
}

/// The board's device tree, dumped by QEMU with the machine options the
/// boot uses (the same -smp, so the CPU list matches). Regenerated every
/// time: it is cheap and must never be stale.
fn aarch64_dtb() -> Vec<u8> {
    let dtb_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/virt-aarch64.dtb");
    // With the firmware, as the boot has it: QEMU builds a board with ACPI
    // tables differently (an ACPI GED where the PL061 would be).
    let (code, _) = aarch64_firmware();
    let status = Command::new("qemu-system-aarch64")
        .arg("-drive")
        .arg(format!("if=pflash,format=raw,unit=0,file={},readonly=on", code.display()))
        .args([
            "-machine",
            &format!("{},dumpdtb=target/virt-aarch64.dtb", aarch64_machine()),
            "-cpu",
            "cortex-a72",
            "-smp",
            "4",
            "-nographic",
            "-serial",
            "none",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("spawn qemu for virt-aarch64.dtb");
    if !status.success() || !dtb_path.is_file() {
        panic!("failed to generate target/virt-aarch64.dtb with qemu-system-aarch64");
    }
    std::fs::read(&dtb_path).expect("read virt-aarch64.dtb")
}


/// rust-lld default image base (AArch64 0x200000, RISC-V 0x10000). netd (and
/// ping) are ET_EXEC; netd has absolute smoltcp vtables. The kernel slides
/// PT_LOAD to USER_BASE without fixing abs relocs, so they must be linked at
/// 0x40000000.
const USER_BASE: u64 = 0x4000_0000;

fn assert_elf_linked_at_user_base(elf: &Path, bin: &str, target: &str) {
    let bytes = std::fs::read(elf).unwrap_or_else(|e| {
        eprintln!("error: read {bin} ELF {}: {e}", elf.display());
        exit(1);
    });
    if bytes.len() < 64 || &bytes[0..4] != b"\x7fELF" {
        eprintln!("error: {bin} for {target} is not ELF64");
        exit(1);
    }
    let entry = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
    let phoff = u64::from_le_bytes(bytes[32..40].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(bytes[54..56].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(bytes[56..58].try_into().unwrap()) as usize;
    let mut min_vaddr = u64::MAX;
    for i in 0..phnum {
        let p = phoff + i * phentsize;
        if p + 40 > bytes.len() {
            break;
        }
        let p_type = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        if p_type != 1 {
            continue;
        }
        let vaddr = u64::from_le_bytes(bytes[p + 16..p + 24].try_into().unwrap());
        min_vaddr = min_vaddr.min(vaddr);
    }
    if entry < USER_BASE || min_vaddr == u64::MAX || min_vaddr < USER_BASE {
        eprintln!(
            "error: {bin} for {target} entry {entry:#x} min PT_LOAD {min_vaddr:#x} below USER_BASE {USER_BASE:#x}"
        );
        exit(1);
    }
}

/// Force a correctly-linked `target/{bin}-$target` before the kernel build.
///
/// Boot CI does `cargo clean -p kernel --target`, but that can leave the
/// kernel build-script fingerprint/output "fresh". The reused USER_*_PATH
/// then `include_bytes!` a rust-cache ELF still linked at 0x200000/0x10000.
fn ensure_user_at_user_base(cargo: &str, target: &str, bin: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target_dir = root.join("target");
    let stable = target_dir.join(format!("{bin}-{target}"));
    let _ = std::fs::remove_file(&stable);

    for profile in ["debug", "release"] {
        let base = target_dir.join(target).join(profile);
        for sub in ["build", ".fingerprint", "incremental"] {
            let dir = base.join(sub);
            if sub == "incremental" {
                let _ = std::fs::remove_dir_all(&dir);
            } else if let Ok(rd) = std::fs::read_dir(&dir) {
                for ent in rd.flatten() {
                    let name = ent.file_name();
                    if name.to_string_lossy().starts_with("kernel-") {
                        let _ = std::fs::remove_dir_all(ent.path());
                        let _ = std::fs::remove_file(ent.path());
                    }
                }
            }
        }
        let host = target_dir.join(profile);
        for sub in ["build", ".fingerprint", "incremental"] {
            let dir = host.join(sub);
            if sub == "incremental" {
                let _ = std::fs::remove_dir_all(&dir);
            } else if let Ok(rd) = std::fs::read_dir(&dir) {
                for ent in rd.flatten() {
                    let name = ent.file_name();
                    if name.to_string_lossy().starts_with("kernel-") {
                        let _ = std::fs::remove_dir_all(ent.path());
                        let _ = std::fs::remove_file(ent.path());
                    }
                }
            }
        }
    }

    // What the crate's build needs first (`PORT_PREPARE`: netd's patched
    // smoltcp), as kernel/build.rs does before its own build of it.
    if let Some(script) = ports::load_all(&root)
        .into_iter()
        .find(|p| p.kind == ports::Kind::User && p.bin == bin)
        .and_then(|p| p.prepare)
    {
        let ok = Command::new("bash")
            .arg(root.join(&script))
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("error: {script} failed for {bin}");
            exit(1);
        }
    }

    let td = target_dir.join(format!("{bin}-prelink-{target}"));
    let _ = std::fs::remove_dir_all(&td);
    let mut rustflags = if target.contains("aarch64") {
        String::from("-C panic=abort -C relocation-model=static")
    } else {
        String::from("-C panic=abort -C relocation-model=static -C code-model=medium")
    };
    rustflags.push_str(" -C link-arg=--image-base -C link-arg=0x40000000");
    let status = Command::new(cargo)
        .args([
            "build",
            "--manifest-path",
            root.join(format!("user/{bin}/Cargo.toml")).to_str().unwrap(),
            "--target",
            target,
            "--bin",
            bin,
            "--target-dir",
        ])
        .arg(&td)
        .env("RUSTFLAGS", &rustflags)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()
        .unwrap_or_else(|e| {
            eprintln!("error: spawn {bin} prelink for {target}: {e}");
            exit(1);
        });
    if !status.success() {
        eprintln!("error: {bin} prelink failed for {target}");
        exit(1);
    }
    let elf = td.join(target).join("debug").join(bin);
    if !elf.is_file() {
        eprintln!("error: {bin} prelink ELF missing at {}", elf.display());
        exit(1);
    }
    assert_elf_linked_at_user_base(&elf, bin, target);
    std::fs::create_dir_all(&target_dir).expect("target dir");
    std::fs::copy(&elf, &stable).unwrap_or_else(|e| {
        eprintln!("error: copy {bin} to {}: {e}", stable.display());
        exit(1);
    });
    stamp_user_into_kernel_outs(&elf, target, bin);
    eprintln!(
        "{bin} prelink ok: {} linked at USER_BASE (min PT_LOAD verified)",
        stable.display()
    );
}

fn stamp_user_into_kernel_outs(good: &Path, target: &str, bin: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let nested_name = format!("{bin}-target");
    for profile in ["debug", "release"] {
        for build_root in [
            root.join(target).join(profile).join("build"),
            root.join(profile).join("build"),
        ] {
            let Ok(rd) = std::fs::read_dir(&build_root) else {
                continue;
            };
            for ent in rd.flatten() {
                if !ent.file_name().to_string_lossy().starts_with("kernel-") {
                    continue;
                }
                let out = ent.path().join("out");
                let nested = out
                    .join(&nested_name)
                    .join(target)
                    .join(profile)
                    .join(bin);
                if nested.is_file() {
                    let _ = std::fs::copy(good, &nested);
                }
                if let Ok(files) = std::fs::read_dir(&out) {
                    for f in files.flatten() {
                        let n = f.file_name();
                        let ns = n.to_string_lossy();
                        if ns.starts_with(&format!("{bin}-")) && ns.ends_with(".elf") {
                            let _ = std::fs::copy(good, f.path());
                        }
                    }
                }
            }
        }
    }
}


/// Boot CI packs aarch64/riscv64 kernels in `ci-build.tar`. When the restore
/// step sets `MYOS_SKIP_KERNEL_REBUILD=1`, use those ELFs instead of
/// `cargo clean` + `cargo build` (was the multi-minute compile inside boot).
fn skip_kernel_rebuild() -> bool {
    matches!(
        std::env::var("MYOS_SKIP_KERNEL_REBUILD").as_deref(),
        Ok("1") | Ok("true")
    )
}

fn prebuilt_kernel(target: &str) -> PathBuf {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
    target_dir.join(target).join(profile).join("kernel")
}

fn build_aarch64_kernel() -> PathBuf {
    let elf = prebuilt_kernel(AARCH64_TARGET);
    if skip_kernel_rebuild() {
        if !elf.is_file() {
            eprintln!(
                "error: MYOS_SKIP_KERNEL_REBUILD set but aarch64 kernel missing at {}",
                elf.display()
            );
            exit(1);
        }
        eprintln!("using prebuilt aarch64 kernel {}", elf.display());
        return elf;
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    // rust-cache can keep a pre--image-base ET_EXEC at 0x200000; cargo clean -p
    // kernel alone may skip build.rs. Prelink ping/netd at USER_BASE and
    // invalidate fingerprints. (CI boot prefers packed kernels via
    // MYOS_SKIP_KERNEL_REBUILD.)
    ensure_user_at_user_base(&cargo, AARCH64_TARGET, "ping");
    ensure_user_at_user_base(&cargo, AARCH64_TARGET, "netd");
    let clean = Command::new(&cargo)
        .args(["clean", "-p", "kernel", "--target", AARCH64_TARGET])
        .status()
        .expect("failed to clean aarch64 kernel");
    if !clean.success() {
        eprintln!("error: cleaning aarch64 kernel failed");
        exit(1);
    }
    let mut cmd = Command::new(&cargo);
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("-p")
        .arg("kernel")
        .arg("--bin")
        .arg("kernel")
        .arg("--target")
        .arg(AARCH64_TARGET);
    if !cfg!(debug_assertions) {
        cmd.arg("--release");
    }
    let status = cmd
        .status()
        .expect("failed to invoke cargo for aarch64 kernel");
    if !status.success() {
        eprintln!("error: building aarch64 kernel failed");
        exit(1);
    }

    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
    let elf = target_dir.join(AARCH64_TARGET).join(profile).join("kernel");
    if !elf.is_file() {
        eprintln!("error: aarch64 kernel ELF missing at {}", elf.display());
        exit(1);
    }
    elf
}

fn run_riscv64() {
    let image = build_riscv64_image();
    let mut cmd = qemu_riscv64(&boot_image(&image), false);
    add_persistent_disk(&mut cmd, "virtio-blk-device");
    let status = cmd.status().expect("failed to start qemu-system-riscv64");
    exit(status.code().unwrap_or(1));
}

fn riscv64_firmware() -> (PathBuf, PathBuf) {
    const CANDIDATES: &[(&str, &str)] = &[
        (
            "/usr/share/qemu-efi-riscv64/RISCV_VIRT_CODE.fd",
            "/usr/share/qemu-efi-riscv64/RISCV_VIRT_VARS.fd",
        ),
        (
            "/usr/share/edk2/riscv64/RISCV_VIRT_CODE.fd",
            "/usr/share/edk2/riscv64/RISCV_VIRT_VARS.fd",
        ),
    ];
    for (c, v) in CANDIDATES {
        if Path::new(c).is_file() {
            return (PathBuf::from(c), PathBuf::from(v));
        }
    }
    panic!(
        "no RISC-V UEFI firmware found. Install qemu-efi-riscv64 (Debian/Ubuntu) \
         or edk2-riscv64"
    );
}

fn run_test_riscv64(mode: Mode) {
    // The image first: its kernel build makes the user programs the
    // packages' boot files (the initramfs) take.
    let image = build_riscv64_image();
    start_package_mirror("riscv64", &mode);
    prepare_alpine_disk("riscv64", &mode);
    test_boots("riscv64", &image, mode, |disk| {
        qemu_riscv64(disk, true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start qemu-system-riscv64")
    });
}

/// Whether this host's `qemu-system-riscv64` has the RISC-V IOMMU
/// (`riscv-iommu-pci`, QEMU 9.2 and later: CI's image has it, Ubuntu
/// 24.04's 8.2 has none). A boot without it has no IOMMU: `/proc/iommu`
/// says `none`, said once here.
fn riscv64_iommu() -> bool {
    static HAS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HAS.get_or_init(|| {
        let out = Command::new("qemu-system-riscv64").args(["-device", "help"]).output();
        let has = out.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("riscv-iommu-pci"));
        if !has {
            eprintln!("note: qemu-system-riscv64 has no riscv-iommu-pci (QEMU 9.2): the boot has no IOMMU");
        }
        has
    })
}

fn qemu_riscv64(image: &Path, ci: bool) -> Command {
    let (code, vars) = riscv64_firmware();
    let mut cmd = Command::new("qemu-system-riscv64");
    // 2G: initramfs + /heap find/cat/ls→rg pressure used to trip sepc=0 under 1G.
    cmd.arg("-global")
        .arg("virtio-mmio.force-legacy=false")
        .arg("-machine")
        .arg("virt");
    // The IOMMU in front of the PCI host, first on the bus (00:01.0), the
    // kernel's `iommu/riscv.rs` on every boot that has it (`/dev/pci`'s
    // devices confined).
    if riscv64_iommu() {
        cmd.arg("-device").arg("riscv-iommu-pci");
    }
    cmd.arg("-cpu")
        .arg("rv64")
        .arg("-m")
        .arg(if ci { "4096" } else { "2048" })
        .arg("-smp")
        // Limine EDK2 path panics with -smp 4: "missing struct riscv_hart for BSP".
        // Keep 2 + dual-hart DTB + parked APs (OpenSBI may pick hartid=1 as BSP).
        // Do not drop CI to -smp 1 to hide the ripgrep sepc=0 IPF — fix the kernel.
        .arg(RISCV_SMP)
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=0,file={},readonly=on",
            code.display()
        ))
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=1,file={},snapshot=on",
            vars.display()
        ));
    add_virtio_blk_riscv64(&mut cmd);
    add_virtio_net(&mut cmd);
    add_edu(&mut cmd);
    add_wifi_radio(&mut cmd);
    cmd.arg("-drive")
        .arg(format!(
            "if=none,id=hd0,format=raw,file={}",
            image.display()
        ))
        .arg("-device")
        .arg("virtio-blk-device,drive=hd0,bootindex=1")
        .arg("-serial")
        .arg("stdio")
        .arg("-nic")
        .arg("none")
        .arg("-no-reboot");
    // Multi-threaded TCG, as on x86 and aarch64: one host thread per hart.
    // Single-threaded TCG (MYOS_TCG_SINGLE=1, local-ci.sh) runs one hart at a
    // time, so an IPI to a halted hart waits for QEMU's 100 ms round-robin
    // kick, and a tickless idle hart made TLB shootdowns that slow.
    if std::env::var("MYOS_TCG_SINGLE").as_deref() == Ok("1") {
        cmd.arg("-accel").arg("tcg,thread=single");
    }
    // The screen in every boot, the headless CI ones too: it is `/dev/fb`
    // (docs/fb.md), which the boot tests draw on.
    cmd.arg("-device").arg("ramfb");
    // The keyboard in every boot too: the boot tests type on it through
    // the monitor (`/dev/console/kbd`, docs/tty.md).
    cmd.arg("-device").arg("virtio-keyboard-device");
    add_pointer(&mut cmd, ci);
    // And a virtio mouse in the test boots, which `user/c/test.sh
    // virtio_mouse` moves (`host.sh mouse virtio`).
    if ci {
        cmd.arg("-device").arg("virtio-mouse-device");
    }
    if ci {
        cmd.arg("-display").arg("none");
        cmd.arg("-monitor").arg(boot_test::monitor_arg());
    }
    cmd
}

fn build_riscv64_image() -> PathBuf {
    let kernel = build_riscv64_kernel();
    let kernel_bytes = std::fs::read(&kernel).expect("read riscv64 kernel ELF");
    let image = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/riscv64.img");
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let esp = esp_limine_files("riscv64");
    let initramfs = initramfs::build_initramfs(&manifest, "riscv64", &esp);
    write_slot_image(&image, &kernel_bytes, &initramfs, esp);
    write_fat_data_image(&fat_img_path());
    image
}

/// riscv64's device tree, dumped by QEMU. Always regenerated with the same
/// `-smp` as qemu_riscv64: a cached single-hart dump (no -smp) only lists
/// cpu@0; when OpenSBI boots on hart 1 Limine panics "missing struct
/// riscv_hart for BSP" before the kernel runs (CI #34824642315).
fn riscv64_dtb() -> Vec<u8> {
    let dtb_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/virt.dtb");
    let status = Command::new("qemu-system-riscv64")
        .args([
            "-machine",
            "virt,dumpdtb=target/virt.dtb",
            "-smp",
            RISCV_SMP,
            "-nographic",
            "-serial",
            "none",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("spawn qemu for virt.dtb");
    if !status.success() || !dtb_path.is_file() {
        panic!("failed to generate target/virt.dtb with qemu-system-riscv64 -smp {RISCV_SMP}");
    }
    std::fs::read(&dtb_path).expect("read virt.dtb")
}

fn build_riscv64_kernel() -> PathBuf {
    let elf = prebuilt_kernel(RISCV64_TARGET);
    if skip_kernel_rebuild() {
        if !elf.is_file() {
            eprintln!(
                "error: MYOS_SKIP_KERNEL_REBUILD set but riscv64 kernel missing at {}",
                elf.display()
            );
            exit(1);
        }
        eprintln!("using prebuilt riscv64 kernel {}", elf.display());
        return elf;
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    // Same rust-cache trap as aarch64 (fault at 0x11e6a).
    ensure_user_at_user_base(&cargo, RISCV64_TARGET, "ping");
    ensure_user_at_user_base(&cargo, RISCV64_TARGET, "netd");
    let clean = Command::new(&cargo)
        .args(["clean", "-p", "kernel", "--target", RISCV64_TARGET])
        .status()
        .expect("failed to clean riscv64 kernel");
    if !clean.success() {
        eprintln!("error: cleaning riscv64 kernel failed");
        exit(1);
    }
    let mut cmd = Command::new(&cargo);
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("-p")
        .arg("kernel")
        .arg("--bin")
        .arg("kernel")
        .arg("--target")
        .arg(RISCV64_TARGET);
    cmd.env(
        "RUSTFLAGS",
        "-C panic=abort -C relocation-model=static -C code-model=large",
    );
    if !cfg!(debug_assertions) {
        cmd.arg("--release");
    }
    let status = cmd
        .status()
        .expect("failed to invoke cargo for riscv64 kernel");
    if !status.success() {
        eprintln!("error: building riscv64 kernel failed");
        exit(1);
    }

    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
    let elf = target_dir.join(RISCV64_TARGET).join(profile).join("kernel");
    if !elf.is_file() {
        eprintln!("error: riscv64 kernel ELF missing at {}", elf.display());
        exit(1);
    }
    elf
}

