// The port descriptors (`port.env`), read by the host build (`build.rs`),
// the initramfs packer and the kernel's build script. The shell side is
// `scripts/ports.sh`; both read the same files. See docs/ports.md.
//
// Included from `build.rs` and `kernel/build.rs` (`include!`) and compiled
// into the host crate (`mod ports`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where a port lives, which decides what the image carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// `ports/`, `user/`, `toolchain/`: in the image.
    Image,
    /// `packages/`: built and published, installed with `get-myos`.
    Package,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A ported program with its own build script (`ports/<name>/build.sh`).
    Port,
    /// A Rust userspace crate kernel/build.rs builds for every arch.
    User,
    /// The C smoke programs.
    C,
    /// The Rust std demo programs.
    Std,
    /// newlib and the Rust std sysroot: what the ports are built with,
    /// built first; their scripts bring them, build.rs does not run them.
    Toolchain,
}

/// One entry of `PORT_FILES`: what the image (or the package) gets.
#[derive(Clone, Debug)]
pub enum FileSpec {
    /// `bin:<target file>:<path>[,<alias>...]`: an executable, aliases share the inode.
    Bin { src: String, paths: Vec<String> },
    /// `data:<target file>:<path>`: a regular file from `target/`.
    Data { src: String, path: String },
    /// `file:<port-dir file>:<path>`: a file checked in next to the descriptor.
    File { src: String, path: String },
    /// `manifest:<target manifest>:<dir>`: `name:path` lines, one executable each.
    Manifest { src: String, dir: String },
    /// `multicall:<target elf>:<target manifest>:<dir>`: one ELF under every name.
    Multicall { elf: String, manifest: String, dir: String },
    /// `tree:<target dir>:<dir>`: a directory copied recursively.
    Tree { src: String, dir: String },
}

#[derive(Clone, Debug)]
pub struct Port {
    pub name: String,
    pub dir: PathBuf,
    pub role: Role,
    pub kind: Kind,
    pub core: bool,
    pub deps: Vec<String>,
    /// Build script, repo-relative (`None`: nothing to build).
    pub build: Option<String>,
    /// Version stamp the build script writes, `target/`-relative
    /// (`PORT_STAMP`, default `.myos-<name>-version`).
    pub stamp: String,
    pub outputs: Vec<String>,
    /// A `target/`-relative file whose presence means the outputs exist
    /// (default: the first output).
    pub ready: Option<String>,
    pub files: Vec<FileSpec>,
    /// Cargo bin name (`User` kind).
    pub bin: String,
    /// binfs path the kernel embeds the program under (`User` kind).
    pub embed: Option<String>,
    /// Link at `USER_BASE` (ET_EXEC with absolute vtables).
    pub image_base: bool,
    /// Extra files whose change rebuilds the program (`User` kind, port-relative).
    pub watch: Vec<String>,
}

impl Port {
    /// The `target/`-relative file that says the outputs exist (for `arch`).
    pub fn ready_file(&self, arch: &str) -> Option<String> {
        let r = self.ready.clone().or_else(|| self.outputs.first().cloned())?;
        Some(expand(&r, arch))
    }
}

/// Expand `{arch}`, `{none}`, `{myos}` and `{kernel}` for one arch.
pub fn expand(s: &str, arch: &str) -> String {
    let kernel = match arch {
        "x86_64" => "x86_64-unknown-none".to_string(),
        "aarch64" => "aarch64-unknown-none-softfloat".to_string(),
        "riscv64" => "riscv64imac-unknown-none-elf".to_string(),
        other => format!("{other}-unknown-none"),
    };
    s.replace("{arch}", arch)
        .replace("{none}", &format!("{arch}-unknown-none"))
        .replace("{myos}", &format!("{arch}-unknown-myos"))
        .replace("{kernel}", &kernel)
}

/// `KEY=value` / `KEY="value"` lines (a quoted value may span lines), `#`
/// comments. No variable expansion: the shell side sources the same file.
fn parse_env(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Some((key, value)) = t.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let value = if let Some(open) = value.strip_prefix('"') {
            let mut v = open.to_string();
            while !v.ends_with('"') {
                let Some(more) = lines.next() else {
                    break;
                };
                v.push(' ');
                v.push_str(more.trim());
            }
            v.trim_end_matches('"').to_string()
        } else {
            value.split('#').next().unwrap_or("").trim().to_string()
        };
        map.insert(key.trim().to_string(), value);
    }
    map
}

fn parse_files(name: &str, spec: &str) -> Vec<FileSpec> {
    let mut out = Vec::new();
    for item in spec.split_whitespace() {
        let parts: Vec<&str> = item.split(':').collect();
        let bad = || panic!("port {name}: bad PORT_FILES entry {item:?}");
        let f = match parts.as_slice() {
            ["bin", src, paths] => FileSpec::Bin {
                src: src.to_string(),
                paths: paths.split(',').map(str::to_string).collect(),
            },
            ["data", src, path] => FileSpec::Data { src: src.to_string(), path: path.to_string() },
            ["file", src, path] => FileSpec::File { src: src.to_string(), path: path.to_string() },
            ["manifest", src, dir] => FileSpec::Manifest { src: src.to_string(), dir: dir.to_string() },
            ["multicall", elf, manifest, dir] => FileSpec::Multicall {
                elf: elf.to_string(),
                manifest: manifest.to_string(),
                dir: dir.to_string(),
            },
            ["tree", src, dir] => FileSpec::Tree { src: src.to_string(), dir: dir.to_string() },
            _ => bad(),
        };
        out.push(f);
    }
    out
}

fn load_one(dir: &Path, role: Role) -> Option<Port> {
    let text = std::fs::read_to_string(dir.join("port.env")).ok()?;
    let env = parse_env(&text);
    let get = |k: &str| env.get(k).cloned().unwrap_or_default();
    let dir_name = dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let name = if get("PORT_NAME").is_empty() { dir_name } else { get("PORT_NAME") };
    let kind = match get("PORT_KIND").as_str() {
        "" | "port" => Kind::Port,
        "user" => Kind::User,
        "c" => Kind::C,
        "std" => Kind::Std,
        "toolchain" => Kind::Toolchain,
        other => panic!("port {name}: unknown PORT_KIND {other:?}"),
    };
    let build = get("PORT_BUILD");
    let build = if build.is_empty() {
        None
    } else if build.contains('/') {
        Some(build)
    } else {
        let rel = dir.to_string_lossy().to_string();
        Some(format!("{rel}/{build}"))
    };
    let list = |k: &str| get(k).split_whitespace().map(str::to_string).collect::<Vec<_>>();
    let bin = if get("PORT_BIN").is_empty() { name.clone() } else { get("PORT_BIN") };
    let stamp = if get("PORT_STAMP").is_empty() {
        format!(".myos-{name}-version")
    } else {
        get("PORT_STAMP")
    };
    Some(Port {
        files: parse_files(&name, &get("PORT_FILES")),
        name,
        dir: dir.to_path_buf(),
        role,
        kind,
        core: get("PORT_CORE") == "1",
        deps: list("PORT_DEPS"),
        build,
        stamp,
        outputs: list("PORT_OUTPUTS"),
        ready: Some(get("PORT_READY")).filter(|s| !s.is_empty()),
        bin,
        embed: Some(get("PORT_EMBED")).filter(|s| !s.is_empty()),
        image_base: get("PORT_IMAGE_BASE") == "1",
        watch: list("PORT_WATCH"),
    })
}

/// Every port, by name. `repo` is the repository root; the `dir` of each
/// port is repo-relative (`ports/vim`), as the scripts print it.
pub fn load_all(repo: &Path) -> Vec<Port> {
    let mut ports: Vec<Port> = Vec::new();
    let roots: [(&str, Role); 4] = [
        ("ports", Role::Image),
        ("user", Role::Image),
        ("toolchain", Role::Image),
        ("packages", Role::Package),
    ];
    for (base, role) in roots {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(repo.join(base)) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    dirs.push(PathBuf::from(base).join(e.file_name()));
                }
            }
        }
        dirs.sort();
        for d in dirs {
            if let Some(p) = load_one(&repo.join(&d), role) {
                let mut p = p;
                p.dir = d;
                ports.push(p);
            }
        }
    }
    ports.sort_by(|a, b| a.name.cmp(&b.name));
    ports
}
