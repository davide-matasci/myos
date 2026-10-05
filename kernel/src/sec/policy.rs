//! The policy language (docs/security.md): users, labels from path rules,
//! domains and their rights, exec transitions. Parsed once per load into
//! tables the checks walk; nothing here looks at processes.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// What a domain may do to a label (a bit set).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Rights(pub u16);

impl Rights {
    pub const NONE: Rights = Rights(0);
    pub const READ: Rights = Rights(1 << 0);
    pub const WRITE: Rights = Rights(1 << 1);
    pub const APPEND: Rights = Rights(1 << 2);
    pub const CREATE: Rights = Rights(1 << 3);
    pub const REMOVE: Rights = Rights(1 << 4);
    pub const EXEC: Rights = Rights(1 << 5);
    pub const SETATTR: Rights = Rights(1 << 6);
    pub const MOUNT: Rights = Rights(1 << 7);
    pub const SIGNAL: Rights = Rights(1 << 8);
    pub const ALL: Rights = Rights((1 << 9) - 1);

    const NAMES: [(&'static str, Rights); 10] = [
        ("read", Rights::READ),
        ("write", Rights::WRITE),
        ("append", Rights::APPEND),
        ("create", Rights::CREATE),
        ("remove", Rights::REMOVE),
        ("exec", Rights::EXEC),
        ("setattr", Rights::SETATTR),
        ("mount", Rights::MOUNT),
        ("signal", Rights::SIGNAL),
        ("all", Rights::ALL),
    ];

    pub fn from_name(name: &str) -> Option<Rights> {
        Rights::NAMES.iter().find(|(n, _)| *n == name).map(|&(_, r)| r)
    }

    /// `read,write` or `read write` (commas or spaces).
    pub fn parse_list(text: &str) -> Option<Rights> {
        let mut r = Rights::NONE;
        for word in text.split(|c: char| c == ',' || c.is_ascii_whitespace()).filter(|w| !w.is_empty()) {
            r = r | Rights::from_name(word)?;
        }
        Some(r)
    }

    pub fn contains(self, other: Rights) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// `write` covers `append`.
    pub fn widened(self) -> Rights {
        if self.contains(Rights::WRITE) { self | Rights::APPEND } else { self }
    }

    /// The names of the rights set, for messages (`read,write`).
    pub fn names(self) -> String {
        let mut out = String::new();
        for (name, r) in Rights::NAMES.iter().take(9) {
            if self.contains(*r) {
                if !out.is_empty() {
                    out.push(',');
                }
                out.push_str(name);
            }
        }
        if out.is_empty() {
            out.push_str("none");
        }
        out
    }
}

impl core::ops::BitOr for Rights {
    type Output = Rights;
    fn bitor(self, o: Rights) -> Rights {
        Rights(self.0 | o.0)
    }
}

impl core::ops::BitAnd for Rights {
    type Output = Rights;
    fn bitand(self, o: Rights) -> Rights {
        Rights(self.0 & o.0)
    }
}

/// One path component of a pattern.
#[derive(Debug)]
enum Seg {
    Lit(String),
    /// `*`: any one component.
    Star,
    /// `**` (last): any number of components, none included.
    Rest,
    /// `$name`: any one component, captured.
    Var(String),
}

/// A path pattern (`/home/$u/**`).
#[derive(Debug)]
pub struct Pattern(Vec<Seg>);

impl Pattern {
    fn parse(text: &str) -> Result<Pattern, String> {
        let Some(rest) = text.strip_prefix('/') else {
            return Err(format!("pattern `{text}` is not absolute"));
        };
        let mut segs = Vec::new();
        let comps: Vec<&str> = rest.split('/').filter(|c| !c.is_empty()).collect();
        for (i, c) in comps.iter().enumerate() {
            segs.push(match *c {
                "**" if i + 1 == comps.len() => Seg::Rest,
                "**" => return Err(format!("`**` must end the pattern `{text}`")),
                "*" => Seg::Star,
                v if v.starts_with('$') && v.len() > 1 => Seg::Var(String::from(&v[1..])),
                lit => Seg::Lit(String::from(lit)),
            });
        }
        Ok(Pattern(segs))
    }

    /// Match `path` (canonical, absolute); the captured `$name`s.
    fn matches<'p>(&self, path: &'p str) -> Option<Vec<(&str, &'p str)>> {
        let comps = path.split('/').filter(|c| !c.is_empty());
        let mut caps = Vec::new();
        let mut comps = comps.peekable();
        for seg in &self.0 {
            match seg {
                Seg::Rest => return Some(caps),
                Seg::Star => {
                    comps.next()?;
                }
                Seg::Lit(l) => {
                    if comps.next()? != l.as_str() {
                        return None;
                    }
                }
                Seg::Var(v) => caps.push((v.as_str(), comps.next()?)),
            }
        }
        if comps.peek().is_some() { None } else { Some(caps) }
    }
}

/// A label rule's parameter: none, a captured `$name`, or a fixed name.
#[derive(Debug)]
enum LabelParam {
    None,
    Var(String),
    Lit(String),
}

#[derive(Debug)]
struct LabelRule {
    pattern: Pattern,
    kind: u16,
    param: LabelParam,
}

/// A file's (or a kernel object's) label: its kind and owner, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub kind: u16,
    pub param: Option<String>,
}

/// The kind of a file no label rule names.
pub const UNLABELED: &str = "unlabeled";

/// Which labels a domain rule covers.
#[derive(Debug)]
enum KindSel {
    Any,
    Kind(u16),
}

/// Which owners a domain rule covers.
#[derive(Debug)]
enum ParamSel {
    /// `kind`: the label has no owner.
    None,
    /// `kind(self)`: the process's user.
    SelfUser,
    /// `kind(group)`: any group of the process's user.
    Group,
    /// `kind(*)`: any owner, or none.
    Any,
    /// `kind(name)`.
    Name(String),
}

#[derive(Debug)]
struct Rule {
    kind: KindSel,
    param: ParamSel,
    rights: Rights,
}

pub struct Domain {
    pub name: String,
    rules: Vec<Rule>,
}

pub struct User {
    pub name: String,
    pub groups: Vec<u16>,
    /// The domains this user's processes may enter by exec.
    pub domains: Vec<u16>,
    /// Where a successful `setuser` puts the process (else the policy's
    /// `login` domain).
    pub login: Option<u16>,
    pub home: String,
    /// `sha256:SALT:HEX` (the digest of SALT followed by the password).
    pub password: Option<(String, [u8; 32])>,
}

struct ExecRule {
    pattern: Pattern,
    domain: u16,
}

pub struct Policy {
    pub enforcing: bool,
    pub users: Vec<User>,
    pub groups: Vec<String>,
    pub domains: Vec<Domain>,
    kinds: Vec<String>,
    labels: Vec<LabelRule>,
    execs: Vec<ExecRule>,
    /// The domain `setuser` enters when the user names none (`login -> d`).
    pub login: Option<u16>,
    /// The first process's user and domain (`boot USER DOMAIN`).
    pub boot: (u16, u16),
}

impl Policy {
    pub fn user_id(&self, name: &str) -> Option<u16> {
        self.users.iter().position(|u| u.name == name).map(|i| i as u16)
    }

    pub fn domain_id(&self, name: &str) -> Option<u16> {
        self.domains.iter().position(|d| d.name == name).map(|i| i as u16)
    }

    pub fn kind_id(&self, name: &str) -> Option<u16> {
        self.kinds.iter().position(|k| k == name).map(|i| i as u16)
    }

    pub fn kind_name(&self, id: u16) -> &str {
        self.kinds.get(id as usize).map_or(UNLABELED, |s| s.as_str())
    }

    /// The label of the file at `path` (canonical, absolute): the last
    /// matching rule's, else [`UNLABELED`].
    pub fn label_of(&self, path: &str) -> Label {
        for rule in self.labels.iter().rev() {
            let Some(caps) = rule.pattern.matches(path) else {
                continue;
            };
            let param = match &rule.param {
                LabelParam::None => None,
                LabelParam::Lit(l) => Some(l.clone()),
                LabelParam::Var(v) => caps.iter().find(|(n, _)| n == v).map(|(_, c)| String::from(*c)),
            };
            return Label { kind: rule.kind, param };
        }
        Label { kind: self.kind_id(UNLABELED).unwrap_or(u16::MAX), param: None }
    }

    /// A kernel object's label (`kernel.modules`, `proc(alice)`).
    pub fn object_label(&self, kind: &str, param: Option<&str>) -> Label {
        Label { kind: self.kind_id(kind).unwrap_or(u16::MAX), param: param.map(String::from) }
    }

    /// What `domain` running for `user` may do to `label`.
    pub fn rights(&self, user: u16, domain: u16, label: &Label) -> Rights {
        let Some(d) = self.domains.get(domain as usize) else {
            return Rights::NONE;
        };
        let u = self.users.get(user as usize);
        let mut r = Rights::NONE;
        for rule in &d.rules {
            let kind_ok = match rule.kind {
                KindSel::Any => true,
                KindSel::Kind(k) => k == label.kind,
            };
            if !kind_ok {
                continue;
            }
            let param_ok = match (&rule.param, &label.param) {
                (ParamSel::Any, _) => true,
                (ParamSel::None, None) => true,
                (ParamSel::SelfUser, Some(p)) => u.is_some_and(|u| &u.name == p),
                (ParamSel::Group, Some(p)) => {
                    u.is_some_and(|u| u.groups.iter().any(|&g| self.groups.get(g as usize) == Some(p)))
                }
                (ParamSel::Name(n), Some(p)) => n == p,
                _ => false,
            };
            // `*` covers every label, whatever its owner.
            if param_ok || matches!(rule.kind, KindSel::Any) {
                r = r | rule.rights;
            }
        }
        r.widened()
    }

    /// The domain an exec of `path` moves `user` into, if a rule names one
    /// that the user may enter.
    pub fn exec_domain(&self, user: u16, path: &str) -> Option<u16> {
        let rule = self.execs.iter().rev().find(|e| e.pattern.matches(path).is_some())?;
        let u = self.users.get(user as usize)?;
        u.domains.contains(&rule.domain).then_some(rule.domain)
    }

    /// The domain `setuser` puts `user` in.
    pub fn login_domain(&self, user: u16) -> Option<u16> {
        self.users.get(user as usize).and_then(|u| u.login).or(self.login)
    }
}

/// Parse a policy (docs/security.md); the error names the line.
pub fn parse(text: &str) -> Result<Policy, String> {
    let mut p = Policy {
        enforcing: true,
        users: Vec::new(),
        groups: Vec::new(),
        domains: Vec::new(),
        kinds: alloc::vec![String::from(UNLABELED)],
        labels: Vec::new(),
        execs: Vec::new(),
        login: None,
        boot: (u16::MAX, u16::MAX),
    };
    // Users, groups and domains may be named before they are defined: the
    // names are collected first, the lines read in a second pass.
    let owned = logical_lines(text);
    let lines: Vec<(usize, &str)> = owned.iter().map(|(n, l)| (*n, l.as_str())).collect();
    for &(_, line) in &lines {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("domain") => {
                let name = words.next().unwrap_or("").trim_end_matches(':');
                if !name.is_empty() && p.domain_id(name).is_none() {
                    p.domains.push(Domain { name: String::from(name), rules: Vec::new() });
                }
            }
            Some("group") => {
                if let Some(g) = words.next() {
                    intern(&mut p.groups, g);
                }
            }
            _ => {}
        }
    }
    let mut boot = None;
    for &(no, line) in &lines {
        let err = |e: String| format!("line {no}: {e}");
        let (head, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        match head {
            "mode" => {
                p.enforcing = match rest {
                    "enforcing" => true,
                    "permissive" => false,
                    _ => return Err(err(format!("mode `{rest}`: enforcing or permissive"))),
                }
            }
            "group" => {}
            "user" => parse_user(&mut p, rest).map_err(err)?,
            "label" => {
                let mut w = rest.split_whitespace();
                let (Some(pat), Some(lab), None) = (w.next(), w.next(), w.next()) else {
                    return Err(err(String::from("label PATTERN KIND[(OWNER)]")));
                };
                let pattern = Pattern::parse(pat).map_err(err)?;
                let (kind, param) = split_param(lab).map_err(err)?;
                let param = match param {
                    None => LabelParam::None,
                    Some(v) if v.starts_with('$') => {
                        let v = &v[1..];
                        if !pattern.0.iter().any(|s| matches!(s, Seg::Var(n) if n == v)) {
                            return Err(err(format!("`${v}` is not in the pattern")));
                        }
                        LabelParam::Var(String::from(v))
                    }
                    Some(l) => LabelParam::Lit(String::from(l)),
                };
                let kind = intern(&mut p.kinds, kind);
                p.labels.push(LabelRule { pattern, kind, param });
            }
            "domain" => {
                let (name, rules) = rest.split_once(':').unwrap_or((rest, ""));
                let d = p.domain_id(name.trim()).ok_or_else(|| err(format!("domain `{name}`")))?;
                let rules = parse_rules(&mut p, rules).map_err(err)?;
                p.domains[d as usize].rules.extend(rules);
            }
            "exec" => {
                let (pat, dom) = rest.split_once("->").ok_or_else(|| err(String::from("exec PATTERN -> DOMAIN")))?;
                let pattern = Pattern::parse(pat.trim()).map_err(err)?;
                let domain = p.domain_id(dom.trim()).ok_or_else(|| err(format!("no domain `{}`", dom.trim())))?;
                p.execs.push(ExecRule { pattern, domain });
            }
            "login" => {
                let dom = rest.strip_prefix("->").map(str::trim).ok_or_else(|| err(String::from("login -> DOMAIN")))?;
                p.login = Some(p.domain_id(dom).ok_or_else(|| err(format!("no domain `{dom}`")))?);
            }
            "boot" => {
                let mut w = rest.split_whitespace();
                let (Some(u), Some(d), None) = (w.next(), w.next(), w.next()) else {
                    return Err(err(String::from("boot USER DOMAIN")));
                };
                boot = Some((u.to_string(), d.to_string(), no));
            }
            _ => return Err(err(format!("unknown line `{head}`"))),
        }
    }
    let Some((u, d, no)) = boot else {
        return Err(String::from("no `boot USER DOMAIN` line"));
    };
    let user = p.user_id(&u).ok_or_else(|| format!("line {no}: no user `{u}`"))?;
    let domain = p.domain_id(&d).ok_or_else(|| format!("line {no}: no domain `{d}`"))?;
    p.boot = (user, domain);
    Ok(p)
}

/// The non-empty lines without comments, an indented line joined to the
/// one before it (a domain's rules), with the number of the first.
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("");
        if line.trim().is_empty() {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        match out.last_mut() {
            Some((_, prev)) if indented => {
                prev.push(' ');
                prev.push_str(line.trim());
            }
            _ => out.push((i + 1, String::from(line.trim()))),
        }
    }
    out
}

fn intern(list: &mut Vec<String>, name: &str) -> u16 {
    match list.iter().position(|n| n == name) {
        Some(i) => i as u16,
        None => {
            list.push(String::from(name));
            (list.len() - 1) as u16
        }
    }
}

/// `kind(param)` → (`kind`, `Some(param)`); `kind` → (`kind`, `None`).
fn split_param(text: &str) -> Result<(&str, Option<&str>), String> {
    match text.split_once('(') {
        None => Ok((text, None)),
        Some((k, rest)) => {
            let p = rest.strip_suffix(')').ok_or_else(|| format!("`{text}`: missing `)`"))?;
            if k.is_empty() || p.is_empty() {
                return Err(format!("`{text}`"));
            }
            Ok((k, Some(p)))
        }
    }
}

/// `user NAME groups: a b domains: c d login: d home: /h password: sha256:S:H`
fn parse_user(p: &mut Policy, rest: &str) -> Result<(), String> {
    let mut words = rest.split_whitespace();
    let name = words.next().ok_or_else(|| String::from("user NAME ..."))?;
    if p.user_id(name).is_some() {
        return Err(format!("user `{name}` twice"));
    }
    let mut user = User {
        name: String::from(name),
        groups: Vec::new(),
        domains: Vec::new(),
        login: None,
        home: String::from("/"),
        password: None,
    };
    let mut key = "";
    for w in words {
        if let Some(k) = w.strip_suffix(':') {
            key = k;
            continue;
        }
        match key {
            "groups" => user.groups.push(intern(&mut p.groups, w)),
            "domains" => user.domains.push(p.domain_id(w).ok_or_else(|| format!("no domain `{w}`"))?),
            "login" => user.login = Some(p.domain_id(w).ok_or_else(|| format!("no domain `{w}`"))?),
            "home" => user.home = String::from(w),
            "password" => user.password = Some(parse_password(w)?),
            _ => return Err(format!("user `{name}`: `{w}` (groups:, domains:, login:, home: or password:)")),
        }
    }
    // The login domain can always be entered.
    if let Some(l) = user.login {
        if !user.domains.contains(&l) {
            user.domains.push(l);
        }
    }
    p.users.push(user);
    Ok(())
}

fn parse_password(text: &str) -> Result<(String, [u8; 32]), String> {
    let mut parts = text.split(':');
    let (Some("sha256"), Some(salt), Some(hex), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(String::from("password: sha256:SALT:HEX"));
    };
    if hex.len() != 64 {
        return Err(String::from("password: the digest is 64 hex digits"));
    }
    let mut d = [0u8; 32];
    for (i, byte) in d.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| String::from("password: not hex"))?;
    }
    Ok((String::from(salt), d))
}

/// `kind(param) {rights} kind {rights} ...`
fn parse_rules(p: &mut Policy, text: &str) -> Result<Vec<Rule>, String> {
    let mut rules = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let open = rest.find('{').ok_or_else(|| format!("`{rest}`: expected `{{rights}}`"))?;
        let close = rest[open..].find('}').map(|i| open + i).ok_or_else(|| format!("`{rest}`: missing `}}`"))?;
        let target = rest[..open].trim();
        let rights = Rights::parse_list(&rest[open + 1..close])
            .ok_or_else(|| format!("`{}`: unknown right", &rest[open..=close]))?;
        let (kind, param) = if target == "*" { ("*", None) } else { split_param(target)? };
        if kind.is_empty() || kind.contains(char::is_whitespace) {
            return Err(format!("`{target}`: one label per `{{rights}}`"));
        }
        let kind = if kind == "*" { KindSel::Any } else { KindSel::Kind(intern(&mut p.kinds, kind)) };
        let param = match param {
            None => ParamSel::None,
            Some("self") => ParamSel::SelfUser,
            Some("group") => ParamSel::Group,
            Some("*") => ParamSel::Any,
            Some(n) => ParamSel::Name(String::from(n)),
        };
        rules.push(Rule { kind, param, rights });
        rest = rest[close + 1..].trim();
    }
    Ok(rules)
}
