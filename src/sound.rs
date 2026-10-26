//! Sound in the QEMU boots (`docs/sound.md`): the virtio-sound cards and
//! a USB speaker, and for the boot tests what they played, checked on the
//! host.
//!
//! An interactive run has a virtio-sound card and the speaker, whose
//! output goes where `MYOS_AUDIODEV` says (a QEMU `-audiodev` driver: `pa`,
//! `pipewire`, `alsa`, `sdl`, ...; `none` without it). A test boot has two
//! cards and the speaker: the first card plays into a WAV file the host
//! reads once QEMU is gone ([`check_played`]) and has no capture stream
//! (the `wav` backend has no input), the second records QEMU's `none`
//! input, silence at the stream's rate; the speaker plays into a WAV file
//! of its own ([`check_usb_played`]).

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// What a test boot's first card plays, `target/boot-test-<name>-sound.wav`.
static WAV: OnceLock<PathBuf> = OnceLock::new();

/// The test boot `name`'s cards play into its own file.
pub fn set_test_name(name: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join(format!("boot-test-{name}-sound.wav"));
    let _ = WAV.set(path);
}

/// The file a test boot's playback card writes.
pub fn wav_path() -> PathBuf {
    WAV.get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("boot-test-sound.wav"))
}

/// The file a test boot's USB speaker writes, beside [`wav_path`]'s.
pub fn usb_wav_path() -> PathBuf {
    wav_path().with_extension("usb.wav")
}

/// A `wav` audiodev `id` writing to `path` at the guest's 48 kHz stereo.
pub fn wav_audiodev(cmd: &mut Command, id: &str, path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    cmd.arg("-audiodev")
        .arg(format!("wav,id={id},path={},out.frequency=48000,out.channels=2,out.format=s16", path.display()));
}

/// The machine's sound cards: virtio-sound on PCI, and QEMU's `usb-audio`
/// speaker on root port 5 of the `qemu-xhci` (every board but the
/// Raspberry Pi's has both).
pub fn add(cmd: &mut Command, test: bool) {
    if test {
        wav_audiodev(cmd, "snd0", &wav_path());
        // Playback only (QEMU's stream 0): the wav backend has no input.
        cmd.arg("-device").arg("virtio-sound-pci,audiodev=snd0,streams=1");
        cmd.arg("-audiodev").arg("none,id=snd1");
        cmd.arg("-device").arg("virtio-sound-pci,audiodev=snd1");
        wav_audiodev(cmd, "snd2", &usb_wav_path());
        cmd.arg("-device").arg(format!("usb-audio,audiodev=snd2,bus={},port=5", crate::XHCI_USB.name));
        return;
    }
    let driver = std::env::var("MYOS_AUDIODEV").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "none".into());
    cmd.arg("-audiodev").arg(format!("{driver},id=snd0"));
    cmd.arg("-device").arg("virtio-sound-pci,audiodev=snd0");
    cmd.arg("-audiodev").arg(format!("{driver},id=snd2"));
    cmd.arg("-device").arg(format!("usb-audio,audiodev=snd2,bus={},port=5", crate::XHCI_USB.name));
}

/// The 16-bit stereo samples of the WAV file at `path` (its `data` chunk,
/// or what follows the header when QEMU left the sizes unwritten), as
/// mono.
fn samples(path: &std::path::Path) -> Option<(u32, Vec<f64>)> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let channels = u16::from_le_bytes([bytes[22], bytes[23]]).max(1) as usize;
    let rate = u32::from_le_bytes(bytes[24..28].try_into().ok()?);
    let bits = u16::from_le_bytes([bytes[34], bytes[35]]);
    if bits != 16 {
        return None;
    }
    let data = &bytes[44..];
    let frame = 2 * channels;
    let mono = data
        .chunks_exact(frame)
        .map(|f| {
            let sum: i32 = f.chunks_exact(2).map(|s| i32::from(i16::from_le_bytes([s[0], s[1]]))).sum();
            f64::from(sum) / channels as f64 / 32768.0
        })
        .collect();
    Some((rate, mono))
}

/// The power of `freq` in `window` (the Goertzel filter), normalized by
/// the window's length.
fn power(window: &[f64], rate: u32, freq: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * freq / f64::from(rate);
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for &x in window {
        let s = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    let p = s1 * s1 + s2 * s2 - coeff * s1 * s2;
    p / (window.len() as f64).powi(2)
}

/// Whether some 100 ms of the file carry each of `tones` (Hz) and only
/// those: each well above the others and the frequencies between them.
fn heard(rate: u32, mono: &[f64], tones: &[f64]) -> bool {
    let probes = [300.0, 523.0, 880.0, 1000.0];
    let window = (rate / 10) as usize;
    mono.chunks_exact(window).any(|w| {
        let floor = probes.iter().filter(|p| !tones.contains(p)).map(|&p| power(w, rate, p)).fold(1e-9, f64::max);
        tones.iter().all(|&t| power(w, rate, t) > 20.0 * floor && power(w, rate, t) > 1e-4)
    })
}

/// After a test boot whose `sound_play` test passed: the WAV file the
/// playback card wrote carries the guest's 440 Hz tone, and (`mixed`,
/// `sound_mix`) 440 and 660 Hz played together through the server.
pub fn check_played(mixed: bool) -> bool {
    let path = wav_path();
    let Some((rate, mono)) = samples(&path) else {
        eprintln!("boot test: sound: no 16-bit WAV at {}", path.display());
        return false;
    };
    let mut ok = true;
    if !heard(rate, &mono, &[440.0]) {
        eprintln!("boot test: sound: no 440 Hz tone in {} ({} frames at {rate} Hz)", path.display(), mono.len());
        ok = false;
    }
    if mixed && !heard(rate, &mono, &[440.0, 660.0]) {
        eprintln!("boot test: sound: no 440 + 660 Hz mix in {}", path.display());
        ok = false;
    }
    if ok {
        eprintln!("boot test: sound: the played tones are in {}", path.display());
    }
    ok
}

/// After a test boot whose `sound_usb` test passed: the USB speaker's WAV
/// file carries the 880 Hz tone the guest played through it.
pub fn check_usb_played() -> bool {
    check_tone(&usb_wav_path(), 880.0, "the USB speaker")
}

/// Whether the WAV file at `path` (what `what` played) carries `freq` Hz.
pub fn check_tone(path: &std::path::Path, freq: f64, what: &str) -> bool {
    let Some((rate, mono)) = samples(path) else {
        eprintln!("boot test: sound: no 16-bit WAV at {}", path.display());
        return false;
    };
    if !heard(rate, &mono, &[freq]) {
        eprintln!("boot test: sound: no {freq} Hz tone in {} ({} frames at {rate} Hz)", path.display(), mono.len());
        return false;
    }
    eprintln!("boot test: sound: {what}'s tone is in {}", path.display());
    true
}
