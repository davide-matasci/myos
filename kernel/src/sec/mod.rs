//! Security (docs/security.md): every process runs for a user in a domain,
//! every file has a label its path gives it, and the policy says what each
//! domain may do to each label. There is no superuser: what the policy does
//! not grant is refused, for every user. A process's namespace
//! (`task::ns`) narrows what it can name and do further.
//!
//! The policy is `/etc/policy`, read at boot (the kernel's own copy when the
//! image has none or it does not parse) and again on `SYS_POLICY_LOAD`.

mod policy;
mod sha256;

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

pub use policy::Rights;
use policy::{Label, Policy};

/// The user and domain a process runs as: indices into the policy's tables.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ctx {
    pub user: u16,
    pub domain: u16,
}

impl Ctx {
    /// The policy's `boot` user and domain (the first process, before the
    /// policy is read).
    pub const BOOT: Ctx = Ctx { user: u16::MAX, domain: u16::MAX };
    /// A user or domain a new policy no longer has: no rights at all.
    const GONE: Ctx = Ctx { user: u16::MAX - 1, domain: u16::MAX - 1 };
}

/// Where the policy lives.
pub const POLICY_PATH: &str = "/etc/policy";
/// The kernel's own copy, used when the image's does not parse.
const BUILTIN: &str = include_str!("../../../etc/policy");

static POLICY: Mutex<Option<Arc<Policy>>> = Mutex::new(None);

fn policy() -> Option<Arc<Policy>> {
    POLICY.lock().clone()
}

/// Read the policy at boot, before the first process starts.
pub fn init() {
    let text = crate::fs::lookup(POLICY_PATH).and_then(|b| core::str::from_utf8(b).ok());
    let p = match text.map(policy::parse) {
        Some(Ok(p)) => p,
        other => {
            if let Some(Err(e)) = other {
                crate::console::status_fail(&format!("{POLICY_PATH}: {e}; the kernel's policy instead"));
            }
            policy::parse(BUILTIN).expect("the kernel's policy parses")
        }
    };
    crate::console::status_ok(if p.enforcing { "security policy (enforcing)" } else { "security policy (permissive)" });
    *POLICY.lock() = Some(Arc::new(p));
}

/// `SYS_POLICY_LOAD`: replace the policy with `text`. The running processes
/// keep their user and domain by name; one the new policy lacks has no
/// rights left.
pub fn load(text: &str) -> Result<(), String> {
    if !allowed_object("kernel.policy", None, Rights::WRITE) {
        return Err(String::from("not allowed"));
    }
    let new = Arc::new(policy::parse(text)?);
    let old = policy();
    crate::task::remap_sec_ctx(|c| {
        let Some(old) = &old else { return c };
        if c == Ctx::BOOT {
            return c;
        }
        let user = old.users.get(c.user as usize).and_then(|u| new.user_id(&u.name));
        let domain = old.domains.get(c.domain as usize).and_then(|d| new.domain_id(&d.name));
        match (user, domain) {
            (Some(user), Some(domain)) => Ctx { user, domain },
            _ => Ctx::GONE,
        }
    });
    *POLICY.lock() = Some(new);
    Ok(())
}

/// The current process's user and domain ids (`None`: the kernel itself, a
/// kernel thread or before the policy is read: no checks).
fn current() -> Option<(Arc<Policy>, u16, u16)> {
    let ctx = crate::task::sec_ctx()?;
    let p = policy()?;
    let (u, d) = if ctx == Ctx::BOOT { p.boot } else { (ctx.user, ctx.domain) };
    Some((p, u, d))
}

/// The path labels are taken from: bind mounts resolved to their source, so
/// a second name cannot give a file a second label.
fn canonical(real: &str) -> String {
    crate::fs::vfs::canonical(real).unwrap_or_else(|| String::from(real))
}

/// A directory a namespace makes up (`task::ns`): it can be listed, no more.
fn synthetic(real: &str) -> bool {
    real.starts_with('@')
}

/// What the current process may do to the file at `real` (a real path):
/// the policy's rights narrowed by its namespace.
pub fn rights_on(real: &str) -> Rights {
    rights_in(real, crate::task::ns_rights(real))
}

/// [`rights_on`] with the namespace's rights replaced by `ns` (those of a
/// directory fd the file was found beneath, `user::at`).
pub fn rights_in(real: &str, ns: Rights) -> Rights {
    if synthetic(real) {
        return Rights::READ;
    }
    match current() {
        None => ns,
        Some((p, u, d)) => p.rights(u, d, &p.label_of(&canonical(real))) & ns,
    }
}

/// May the current process do `need` to the file at `real`? A refusal by
/// the policy is logged (and let through in permissive mode); one by the
/// namespace is not: the process cannot name the file.
pub fn allowed(real: &str, need: Rights) -> bool {
    allowed_in(real, need, crate::task::ns_rights(real))
}

/// [`allowed`] with the namespace's rights replaced by `ns` (those of a
/// directory fd the file was found beneath, `user::at`).
pub fn allowed_in(real: &str, need: Rights, ns: Rights) -> bool {
    if synthetic(real) {
        return Rights::READ.contains(need);
    }
    if !ns.contains(need) {
        return false;
    }
    let Some((p, u, d)) = current() else {
        return true;
    };
    let label = p.label_of(&canonical(real));
    p.rights(u, d, &label).contains(need) || refused(&p, u, d, real, &label, need)
}

/// May the current process rename the file at `old` to `new` (real paths)?
/// A move removes the old name and creates the new one (`replaced`: it
/// removes a file there too), and for a directory every name beneath it
/// moves with it, to a path the label rules may read differently (a home
/// moved out of `/home` is a home no more): each needs `remove` where it
/// is and `create` where it goes, so a rename grants its caller nothing it
/// could not get by creating and removing the names itself. `old_ns` and
/// `new_ns` stand in for the namespace's rights beneath a directory fd the
/// name was found through (`user::at`).
pub fn may_rename(old: &str, old_ns: Option<Rights>, new: &str, new_ns: Option<Rights>, replaced: bool) -> bool {
    let ns = |real: &str, fixed: Option<Rights>| fixed.unwrap_or_else(|| crate::task::ns_rights(real));
    let need_new = if replaced { Rights::CREATE | Rights::REMOVE } else { Rights::CREATE };
    if !allowed_in(old, Rights::REMOVE, ns(old, old_ns)) || !allowed_in(new, need_new, ns(new, new_ns)) {
        return false;
    }
    if !crate::fs::stat(old).is_some_and(|st| st.mode & crate::fs::S_IFMT == S_IFDIR) {
        return true;
    }
    // The names beneath `old`, depth first, without the stack growing with
    // the tree's depth.
    let mut todo: alloc::vec::Vec<String> = alloc::vec![String::new()];
    let mut names = alloc::vec![0u8; RENAME_LIST_MAX];
    while let Some(rel) = todo.pop() {
        let dir = format!("{old}{rel}");
        let n = crate::fs::listdir(&dir, &mut names);
        // A listing the buffer could not hold: not every name was checked.
        if n == names.len() || (n > 0 && names[n - 1] != b'\n') {
            return false;
        }
        for name in names[..n].split(|&b| b == b'\n').filter(|s| !s.is_empty()) {
            let Ok(name) = core::str::from_utf8(name) else {
                return false;
            };
            let sub = format!("{rel}/{name}");
            let (from, to) = (format!("{old}{sub}"), format!("{new}{sub}"));
            if !allowed_in(&from, Rights::REMOVE, ns(&from, old_ns))
                || !allowed_in(&to, Rights::CREATE, ns(&to, new_ns))
            {
                return false;
            }
            if crate::fs::stat(&from).is_some_and(|st| st.mode & crate::fs::S_IFMT == S_IFDIR) {
                todo.push(sub);
            }
        }
    }
    true
}

/// The longest listing of one directory [`may_rename`] walks (names and
/// newlines).
const RENAME_LIST_MAX: usize = 256 * 1024;
/// A directory, in a `stat` mode.
const S_IFDIR: u32 = 0o040000;

/// May the current process do `need` to a kernel object (`kernel.modules`,
/// `kernel.clock`, `kernel.policy`, `kernel.users`, `proc(alice)`)?
pub fn allowed_object(kind: &str, param: Option<&str>, need: Rights) -> bool {
    let Some((p, u, d)) = current() else {
        return true;
    };
    let label = p.object_label(kind, param);
    p.rights(u, d, &label).contains(need) || refused(&p, u, d, kind, &label, need)
}

/// May the current process signal a process running as `target`?
pub fn may_signal(target: Ctx) -> bool {
    let Some(p) = policy() else {
        return true;
    };
    let user = if target == Ctx::BOOT { p.boot.0 } else { target.user };
    let name = p.users.get(user as usize).map(|u| u.name.as_str());
    allowed_object("proc", name, Rights::SIGNAL)
}

static LOG_SECOND: AtomicU64 = AtomicU64::new(0);
static LOG_COUNT: AtomicU32 = AtomicU32::new(0);
/// Refusals logged per second; the rest are counted, not shown.
const LOG_PER_SECOND: u32 = 8;

/// Log a refusal (a few per second); in permissive mode, let it through.
fn refused(p: &Policy, u: u16, d: u16, what: &str, label: &Label, need: Rights) -> bool {
    let second = crate::time::monotonic_ns() / 1_000_000_000;
    if LOG_SECOND.swap(second, Ordering::Relaxed) != second {
        LOG_COUNT.store(0, Ordering::Relaxed);
    }
    if LOG_COUNT.fetch_add(1, Ordering::Relaxed) < LOG_PER_SECOND {
        let user = p.users.get(u as usize).map_or("?", |x| x.name.as_str());
        let domain = p.domains.get(d as usize).map_or("?", |x| x.name.as_str());
        let kind = p.kind_name(label.kind);
        let label = match &label.param {
            Some(o) => format!("{kind}({o})"),
            None => String::from(kind),
        };
        let verb = if p.enforcing { "denied" } else { "would deny" };
        crate::console::write_str(&format!(
            "sec: {verb} {user}/{domain} {} on {what} [{label}]\n",
            need.names()
        ));
    }
    !p.enforcing
}

/// `setuser(name, password)`: run the calling process as `name`, in that
/// user's login domain. A user with a password needs it; one without can
/// only be entered by a domain with `write` on `kernel.users` (login).
pub fn setuser(name: &str, password: &[u8]) -> bool {
    let Some(p) = policy() else {
        return false;
    };
    let Some(uid) = p.user_id(name) else {
        return false;
    };
    let ok = match &p.users[uid as usize].password {
        Some((salt, want)) => {
            let got = sha256::digest(&[salt.as_bytes(), password]);
            // The same time whatever the first differing byte.
            got.iter().zip(want).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
        }
        None => allowed_object("kernel.users", None, Rights::WRITE),
    };
    let Some(domain) = p.login_domain(uid).filter(|_| ok) else {
        return false;
    };
    crate::task::set_sec_ctx(Ctx { user: uid, domain });
    true
}

/// The user and domain an exec of `real` gives the current process, when
/// an `exec` rule names a domain its user may enter (`None`: unchanged).
pub fn exec_ctx(real: &str) -> Option<Ctx> {
    let ctx = crate::task::sec_ctx()?;
    let p = policy()?;
    let user = if ctx == Ctx::BOOT { p.boot.0 } else { ctx.user };
    let domain = p.exec_domain(user, &canonical(real))?;
    Some(Ctx { user, domain })
}

/// The permission bits `stat` shows for `real`: the owner's are what the
/// current process may do (`r` read, `w` write, `x` exec, or for a
/// directory read), the group's and others' are clear.
pub fn mode_bits(real: &str, is_dir: bool) -> u32 {
    mode_bits_in(real, is_dir, crate::task::ns_rights(real))
}

/// [`mode_bits`] with the namespace's rights replaced by `ns`.
pub fn mode_bits_in(real: &str, is_dir: bool, ns: Rights) -> u32 {
    let r = rights_in(real, ns);
    let mut m = 0;
    if r.contains(Rights::READ) {
        m |= 0o400;
    }
    if r.contains(Rights::WRITE) || (is_dir && r.contains(Rights::CREATE)) {
        m |= 0o200;
    }
    if r.contains(Rights::EXEC) || (is_dir && r.contains(Rights::READ)) {
        m |= 0o100;
    }
    m
}

/// The uid `stat` shows for `real`: its label's owner when that is a user
/// (`home(alice)`), else 0.
pub fn owner_uid(real: &str) -> u32 {
    if synthetic(real) {
        return 0;
    }
    let Some(p) = policy() else {
        return 0;
    };
    let label = p.label_of(&canonical(real));
    label.param.and_then(|o| p.user_id(&o)).map_or(0, u32::from)
}

/// `/proc/self/ctx`: `uid user domain`.
pub fn ctx_text() -> String {
    match current() {
        Some((p, u, d)) => {
            let user = p.users.get(u as usize).map_or("-", |x| x.name.as_str());
            let domain = p.domains.get(d as usize).map_or("-", |x| x.name.as_str());
            format!("{u} {user} {domain}\n")
        }
        None => String::from("- kernel kernel\n"),
    }
}

/// `/proc/sys/security/users`: `uid name home group,group`, one per user.
pub fn users_text() -> String {
    let mut out = String::new();
    if let Some(p) = policy() {
        for (i, u) in p.users.iter().enumerate() {
            let groups: alloc::vec::Vec<&str> =
                u.groups.iter().filter_map(|&g| p.groups.get(g as usize).map(|s| s.as_str())).collect();
            out.push_str(&format!("{i} {} {} {}\n", u.name, u.home, groups.join(",")));
        }
    }
    out
}
