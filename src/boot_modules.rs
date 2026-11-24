// The kernel modules of the images: which the kernel loads at boot, which
// ship for `insmod`. Shared by the launcher and `build.rs` (through
// `src/limine_image.rs`) and by the packer myos runs on itself
// (`user/mkinitramfs`).

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
    "rpi_firmware",
    "virtio_blk",
    "virtio_scsi",
    "nvme",
    "sdmmc",
    "sound",
    "virtio_sound",
    "usb",
    "xhci",
    "ehci",
    "uhci",
    "ohci",
    "dwc2",
    "usb_hub",
    "usb_storage",
    "usb_hid",
    "usb_audio",
    "usbdev",
    "virtio_net",
    "tap",
    "netfs",
    "fat",
    "ext2",
];

/// Modules shipped under `/lib/modules/<name>` for `insmod` only, never
/// loaded at boot: `pcidev`, the userspace PCI drivers' `/dev/pci`, which
/// a board without an IOMMU cannot confine (issue #501), so root opts in;
/// `pl061`, a GPIO controller's interrupt domain no driver uses yet (its
/// test loads it).
pub const RUNTIME_MODULES: &[&str] = &["pcidev", "pl061"];

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
        .chain(RUNTIME_MODULES.iter().copied())
        .chain(OPTIONAL_MODULES.iter().map(|(m, _)| *m))
        .collect()
}

/// `/lib/modules/boot.list` in the initramfs: [`boot_modules`], one per
/// line, in load order.
pub fn boot_list() -> String {
    boot_modules().iter().map(|m| format!("{m}\n")).collect()
}
