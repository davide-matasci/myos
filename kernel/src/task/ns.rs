//! Per-process namespaces (docs/security.md): what a process can name.
//!
//! Without one a process sees the whole tree. With one it sees only its
//! bindings: each puts a real directory or file (`source`) at a path of the
//! process's own tree (`target`), with the rights that can be used through
//! it. A path under no binding does not exist; a directory above bindings
//! (`/` when only `/bin` is bound) is made up and only lists them. A
//! namespace is inherited on fork, kept across exec, and only ever narrowed
//! (`SYS_NS`, `chroot`): a binding's source is named in the caller's own
//! namespace, with at most the caller's rights there.

use alloc::string::String;
use alloc::vec::Vec;

use crate::sec::Rights;

pub struct Binding {
    /// The path in the process's tree (canonical, absolute).
    pub target: String,
    /// The real path behind it (canonical, absolute).
    pub source: String,
    pub rights: Rights,
}

pub struct Namespace {
    pub binds: Vec<Binding>,
}

/// Where a path of the process's tree leads.
pub enum Mapped {
    /// A real path.
    Real(String),
    /// A directory above bindings: listable, nothing else.
    Synthetic,
}

/// `path` relative to `prefix` (`""`, or starting with `/`), if it lies at
/// or below it component-wise.
fn below<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix == "/" {
        return Some(if path == "/" { "" } else { path });
    }
    let rest = path.strip_prefix(prefix)?;
    (rest.is_empty() || rest.starts_with('/')).then_some(rest)
}

fn join(base: &str, rest: &str) -> String {
    if rest.is_empty() {
        return String::from(base);
    }
    if base == "/" {
        return String::from(rest);
    }
    let mut s = String::from(base);
    s.push_str(rest);
    s
}

impl Clone for Namespace {
    fn clone(&self) -> Namespace {
        Namespace {
            binds: self
                .binds
                .iter()
                .map(|b| Binding { target: b.target.clone(), source: b.source.clone(), rights: b.rights })
                .collect(),
        }
    }
}

impl Namespace {
    /// Where `virt` (canonical, absolute) leads: the binding with the
    /// longest target above it, else a made-up directory above bindings.
    pub fn map(&self, virt: &str) -> Option<Mapped> {
        let best = self
            .binds
            .iter()
            .filter_map(|b| below(virt, &b.target).map(|rest| (b, rest)))
            .max_by_key(|(b, _)| b.target.len());
        if let Some((b, rest)) = best {
            return Some(Mapped::Real(join(&b.source, rest)));
        }
        self.binds.iter().any(|b| below(&b.target, virt).is_some()).then_some(Mapped::Synthetic)
    }

    /// The rights through the bindings that lead to `real`: what any name
    /// of the file in this tree allows.
    pub fn rights(&self, real: &str) -> Rights {
        self.binds
            .iter()
            .filter(|b| below(real, &b.source).is_some())
            .fold(Rights::NONE, |r, b| r | b.rights)
    }

    /// The names a made-up directory `virt` lists: the next component of
    /// each binding target below it.
    pub fn children(&self, virt: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for b in &self.binds {
            let Some(rest) = below(&b.target, virt) else {
                continue;
            };
            let Some(name) = rest.trim_start_matches('/').split('/').next().filter(|n| !n.is_empty()) else {
                continue;
            };
            if !out.iter().any(|n| n == name) {
                out.push(String::from(name));
            }
        }
        out
    }

    /// The process's name for `real`, if a binding leads to it (the longest
    /// source wins).
    pub fn to_virtual(&self, real: &str) -> Option<String> {
        let (b, rest) = self
            .binds
            .iter()
            .filter_map(|b| below(real, &b.source).map(|rest| (b, rest)))
            .max_by_key(|(b, _)| b.source.len())?;
        Some(join(&b.target, rest))
    }
}
