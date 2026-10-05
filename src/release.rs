// The release a build belongs to, for the packages (`src/packages.rs`) and
// the image (`lib/myos-release` in the initramfs): what `get-myos` compares
// to know whether a mirror's packages fit the running system.
//
// ```text
// release=202610050258 commit=edad890b abi=61
// ```
//
// - `release`: the committer date of `HEAD` in UTC, `YYYYMMDDHHMM`, so a
//   later build has the greater number (`0` outside a git checkout);
// - `commit`: the short hash, for people;
// - `abi`: the native syscall ABI, the count of syscall numbers the kernel
//   knows (`kernel/src/user/syscall.rs`). The numbers only grow
//   (`AGENTS.md`), so a package built against `abi=N` runs on any kernel
//   with `abi >= N`, and `get-myos` refuses an index whose `abi` is above
//   the system's: its programs could call syscalls the kernel lacks.

use std::path::Path;
use std::process::Command;

pub struct Release {
    pub id: String,
    pub commit: String,
    pub abi: usize,
}

impl Release {
    /// The build's release, from the checkout and the kernel source.
    pub fn current(manifest_dir: &Path) -> Release {
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(manifest_dir)
                .args(args)
                .env("TZ", "UTC")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let id = git(&["log", "-1", "--date=format-local:%Y%m%d%H%M", "--format=%cd"])
            .unwrap_or_else(|| "0".to_string());
        let commit = git(&["rev-parse", "--short=8", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
        Release { id, commit, abi: syscall_abi(manifest_dir) }
    }

    /// The text of `lib/myos-release` and of the index header's fields.
    pub fn text(&self) -> String {
        format!("release={} commit={} abi={}\n", self.id, self.commit, self.abi)
    }
}

/// One more than the highest native syscall number the kernel defines.
fn syscall_abi(manifest_dir: &Path) -> usize {
    let src = std::fs::read_to_string(manifest_dir.join("kernel/src/user/syscall.rs"))
        .expect("read kernel/src/user/syscall.rs for the syscall ABI");
    let mut max = None;
    for line in src.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("const SYS_") else {
            continue;
        };
        let Some((_, value)) = rest.split_once(": usize = ") else {
            continue;
        };
        let n: usize = value.trim_end_matches(';').trim().parse().expect("syscall number");
        max = Some(max.map_or(n, |m: usize| m.max(n)));
    }
    max.expect("no syscall numbers in kernel/src/user/syscall.rs") + 1
}
