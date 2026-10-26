//! USB 2.0 the Intel way in the QEMU boots (`docs/usb.md`): an ICH9 EHCI
//! with its three UHCI companions, as many PCs have and UTM's "USB 2.0"
//! setting makes, beside the xHCI. Its high-speed devices are the `ehci`
//! module's; the full- and low-speed ones go to a companion, the `uhci`
//! module's.
//!
//! A test boot puts a high-speed device on the EHCI (QEMU's MTP
//! responder, `usb-mtp`, which no kernel driver takes: `usbdev`'s) and a
//! full-speed hub, a speaker behind it, on its port 3, which the EHCI
//! hands to the second companion: the speaker plays into a WAV file of
//! its own ([`check_played`]).

use std::path::PathBuf;
use std::process::Command;

/// The EHCI's bus.
const BUS: &str = "ehci.0";

/// The file a test boot's UHCI speaker writes, beside the other cards'.
pub fn wav_path() -> PathBuf {
    crate::sound::wav_path().with_extension("uhci.wav")
}

/// The controllers, and in a test boot their devices.
pub fn add(cmd: &mut Command, test: bool) {
    cmd.arg("-device").arg("ich9-usb-ehci1,id=ehci");
    for (i, first) in [(1, 0), (2, 2), (3, 4)] {
        cmd.arg("-device").arg(format!("ich9-usb-uhci{i},masterbus={BUS},firstport={first},multifunction=on"));
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
}

/// After a test boot whose `sound_usb` test passed: the UHCI speaker's
/// WAV file carries the 880 Hz tone the guest played through every
/// speaker.
pub fn check_played() -> bool {
    crate::sound::check_tone(&wav_path(), 880.0, "the UHCI speaker")
}
