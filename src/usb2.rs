//! USB 2.0 in the QEMU boots (`docs/usb.md`), beside the xHCI, both ways
//! PCs have it: an ICH9 EHCI with three UHCI companions (Intel's; what
//! UTM's "USB 2.0" setting makes) and a second EHCI with two OHCI
//! companions (AMD's, NVIDIA's and many SoCs' pairing). Their high-speed
//! devices are the `ehci` module's; the full- and low-speed ones go to a
//! companion, the `uhci` or the `ohci` module's.
//!
//! A test boot puts a high-speed device on the first EHCI (QEMU's MTP
//! responder, `usb-mtp`, which no kernel driver takes: `usbdev`'s) and a
//! full-speed hub, a speaker behind it, on its port 3, which the EHCI
//! hands to the second UHCI. The second EHCI gets the same hub and
//! speaker on its port 1 (the first OHCI's) and a smartcard reader
//! (`usb-ccid`, `usbdev`'s too) on its port 4 (the second OHCI's). Each
//! speaker plays into a WAV file of its own ([`check_played`]).

use std::path::PathBuf;
use std::process::Command;

/// The EHCIs' buses.
const BUS: &str = "ehci.0";
const BUS2: &str = "ehci2.0";

/// The file a test boot's UHCI speaker writes, beside the other cards'.
pub fn wav_path() -> PathBuf {
    crate::sound::wav_path().with_extension("uhci.wav")
}

/// The OHCI speaker's.
pub fn ohci_wav_path() -> PathBuf {
    crate::sound::wav_path().with_extension("ohci.wav")
}

/// The controllers, and in a test boot their devices.
pub fn add(cmd: &mut Command, test: bool) {
    cmd.arg("-device").arg("ich9-usb-ehci1,id=ehci");
    for (i, first) in [(1, 0), (2, 2), (3, 4)] {
        cmd.arg("-device").arg(format!("ich9-usb-uhci{i},masterbus={BUS},firstport={first},multifunction=on"));
    }
    // QEMU's OHCI companions any EHCI that takes companions (the ICH9
    // ones): three ports each.
    cmd.arg("-device").arg("ich9-usb-ehci2,id=ehci2");
    for first in [0, 3] {
        cmd.arg("-device").arg(format!("pci-ohci,masterbus={BUS2},firstport={first},num-ports=3"));
    }
    if !test {
        return;
    }
    // The MTP responder shares a directory: a file in it, read-only.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("usb-mtp");
    let _ = std::fs::create_dir_all(&root);
    let _ = std::fs::write(root.join("hello.txt"), "myos\n");
    cmd.arg("-device").arg(format!("usb-mtp,rootdir={},readonly=on,bus={BUS},port=1", root.display()));
    cmd.arg("-device").arg(format!("usb-hub,bus={BUS},port=3,id=ehcihub"));
    crate::sound::wav_audiodev(cmd, "snd3", &wav_path());
    cmd.arg("-device").arg(format!("usb-audio,audiodev=snd3,bus={BUS},port=3.1"));
    cmd.arg("-device").arg(format!("usb-hub,bus={BUS2},port=1,id=ehci2hub"));
    crate::sound::wav_audiodev(cmd, "snd4", &ohci_wav_path());
    cmd.arg("-device").arg(format!("usb-audio,audiodev=snd4,bus={BUS2},port=1.1"));
    cmd.arg("-device").arg(format!("usb-ccid,bus={BUS2},port=4"));
}

/// After a test boot whose `sound_usb` test passed: the UHCI and OHCI
/// speakers' WAV files carry the 880 Hz tone the guest played through
/// every speaker.
pub fn check_played() -> bool {
    crate::sound::check_tone(&wav_path(), 880.0, "the UHCI speaker")
        && crate::sound::check_tone(&ohci_wav_path(), 880.0, "the OHCI speaker")
}
