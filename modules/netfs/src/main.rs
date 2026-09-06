//! Plan 9 `/net` plus `/dev/netd` channel to userspace netd.
//!
//! Lookup always fails; bytes go through the ABI v7 `read`/`write` hooks.
//! Syscalls run with interrupts off — never busy-spin, never sleep.

#![no_std]
#![no_main]

use myos_abi::{status_ok, ABI_VERSION, KernelApi, ModuleChrOps, ModuleVfsOps, VfsStatInfo};

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

const PROTO_TCP: u8 = 1;
const PROTO_UDP: u8 = 2;
const PROTO_ICMP: u8 = 3;

const REQ_CLONE: u8 = 1;
const REQ_CTL: u8 = 2;
const REQ_SEND: u8 = 3;
const REQ_CLOSE: u8 = 4;

const REP_CLONE_OK: u8 = 1;
const REP_DATA: u8 = 2;
const REP_STATUS: u8 = 3;
const REP_ERR: u8 = 4;

const REQ_HDR: usize = 6;
const REP_HDR: usize = 9;
/// Cap matches kernel `FILE_IO_TMP` (2048). TLS ClientHello / cert fragments
/// need more than the old 512-byte slots (HTTPS handshake timed out in CI).
const MSG_CAP: usize = 2048;
const RING: usize = 8;
const MAX_CONV: usize = 8;
/// Per-conversation RX staging. Cert chains exceed 512; drop = TLS timeout.
const DATA_CAP: usize = 8192;
const STATUS_CAP: usize = 64;

#[derive(Clone, Copy)]
struct Msg {
    len: u16,
    buf: [u8; MSG_CAP],
}

impl Msg {
    const EMPTY: Self = Self {
        len: 0,
        buf: [0; MSG_CAP],
    };
}

struct RingBuf {
    slots: [Msg; RING],
    head: u8,
    tail: u8,
    count: u8,
}

impl RingBuf {
    const EMPTY: Self = Self {
        slots: [Msg::EMPTY; RING],
        head: 0,
        tail: 0,
        count: 0,
    };

    fn push(&mut self, src: &[u8]) -> bool {
        if self.count as usize >= RING || src.is_empty() || src.len() > MSG_CAP {
            return false;
        }
        let i = self.head as usize % RING;
        self.slots[i].len = src.len() as u16;
        self.slots[i].buf[..src.len()].copy_from_slice(src);
        self.head = self.head.wrapping_add(1);
        self.count += 1;
        true
    }

    fn pop(&mut self, dst: &mut [u8]) -> usize {
        if self.count == 0 {
            return 0;
        }
        let i = self.tail as usize % RING;
        let n = (self.slots[i].len as usize).min(dst.len()).min(MSG_CAP);
        if n != 0 {
            dst[..n].copy_from_slice(&self.slots[i].buf[..n]);
        }
        self.slots[i].len = 0;
        self.tail = self.tail.wrapping_add(1);
        self.count -= 1;
        n
    }
}

struct Conv {
    used: bool,
    proto: u8,
    data_len: u16,
    data: [u8; DATA_CAP],
    status_len: u16,
    status: [u8; STATUS_CAP],
}

impl Conv {
    const EMPTY: Self = Self {
        used: false,
        proto: 0,
        data_len: 0,
        data: [0; DATA_CAP],
        status_len: 0,
        status: [0; STATUS_CAP],
    };
}

struct State {
    req: RingBuf,
    convs: [Conv; MAX_CONV],
}

static mut STATE: State = State {
    req: RingBuf::EMPTY,
    convs: [Conv::EMPTY; MAX_CONV],
};

fn state() -> &'static mut State {
    unsafe { &mut *core::ptr::addr_of_mut!(STATE) }
}

fn proto_name(p: u8) -> Option<&'static str> {
    match p {
        PROTO_TCP => Some("tcp"),
        PROTO_UDP => Some("udp"),
        PROTO_ICMP => Some("icmp"),
        _ => None,
    }
}

fn parse_proto(s: &str) -> Option<u8> {
    match s {
        "tcp" => Some(PROTO_TCP),
        "udp" => Some(PROTO_UDP),
        "icmp" => Some(PROTO_ICMP),
        _ => None,
    }
}

fn parse_u16(s: &str) -> Option<u16> {
    if s.is_empty() || s.len() > 5 {
        return None;
    }
    let mut n = 0u32;
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    if n > 65535 { None } else { Some(n as u16) }
}

#[derive(Clone, Copy)]
enum Node {
    Root,
    Proto(u8),
    Clone(u8),
    ConvDir(u8, u16),
    Ctl(u8, u16),
    Data(u8, u16),
    Status(u8, u16),
}

fn parse_path(path: &str) -> Option<Node> {
    if path.is_empty() || path == "." || path == ".." {
        return Some(Node::Root);
    }
    let mut it = path.split('/');
    let a = it.next()?;
    let proto = parse_proto(a)?;
    match it.next() {
        None => Some(Node::Proto(proto)),
        Some("clone") => {
            if it.next().is_some() {
                None
            } else {
                Some(Node::Clone(proto))
            }
        }
        Some(id) => {
            let conv = parse_u16(id)?;
            match it.next() {
                None => Some(Node::ConvDir(proto, conv)),
                Some("ctl") if it.next().is_none() => Some(Node::Ctl(proto, conv)),
                Some("data") if it.next().is_none() => Some(Node::Data(proto, conv)),
                Some("status") if it.next().is_none() => Some(Node::Status(proto, conv)),
                _ => None,
            }
        }
    }
}

fn put_bytes(dst: &mut [u8], n: &mut usize, s: &[u8]) -> bool {
    let need = s.len() + 1;
    if *n + need > dst.len() {
        return false;
    }
    dst[*n..*n + s.len()].copy_from_slice(s);
    *n += s.len();
    dst[*n] = b'\n';
    *n += 1;
    true
}

fn put_dec(dst: &mut [u8], n: &mut usize, v: u16) -> bool {
    let mut tmp = [0u8; 6];
    let mut x = v;
    let mut i = tmp.len();
    if x == 0 {
        i -= 1;
        tmp[i] = b'0';
    } else {
        while x != 0 && i > 0 {
            i -= 1;
            tmp[i] = b'0' + (x % 10) as u8;
            x /= 10;
        }
    }
    put_bytes(dst, n, &tmp[i..])
}

fn encode_req(typ: u8, conv: u16, proto: u8, payload: &[u8], out: &mut [u8]) -> Option<usize> {
    let n = REQ_HDR + payload.len();
    if n > out.len() || payload.len() > MSG_CAP - REQ_HDR {
        return None;
    }
    out[0] = typ;
    out[1..3].copy_from_slice(&conv.to_le_bytes());
    out[3] = proto;
    out[4..6].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    if !payload.is_empty() {
        out[6..n].copy_from_slice(payload);
    }
    Some(n)
}

fn enqueue_req(typ: u8, conv: u16, proto: u8, payload: &[u8]) -> bool {
    let mut tmp = [0u8; MSG_CAP];
    let Some(n) = encode_req(typ, conv, proto, payload, &mut tmp) else {
        return false;
    };
    state().req.push(&tmp[..n])
}

fn alloc_conv(proto: u8) -> Option<u16> {
    let st = state();
    for i in 0..MAX_CONV {
        if !st.convs[i].used {
            st.convs[i] = Conv {
                used: true,
                proto,
                data_len: 0,
                data: [0; DATA_CAP],
                status_len: 0,
                status: [0; STATUS_CAP],
            };
            return Some(i as u16);
        }
    }
    None
}

fn conv_ok(id: u16, proto: u8) -> bool {
    let i = id as usize;
    let st = state();
    i < MAX_CONV && st.convs[i].used && st.convs[i].proto == proto
}

fn conv_mut(id: u16) -> Option<&'static mut Conv> {
    let i = id as usize;
    let st = state();
    if i >= MAX_CONV || !st.convs[i].used {
        None
    } else {
        Some(&mut st.convs[i])
    }
}

fn set_status(c: &mut Conv, s: &[u8]) {
    let n = s.len().min(STATUS_CAP);
    c.status[..n].copy_from_slice(&s[..n]);
    c.status_len = n as u16;
}

fn append_data(c: &mut Conv, src: &[u8]) {
    let have = c.data_len as usize;
    let n = src.len().min(DATA_CAP.saturating_sub(have));
    if n == 0 {
        return;
    }
    c.data[have..have + n].copy_from_slice(&src[..n]);
    c.data_len = (have + n) as u16;
}

fn apply_reply(buf: &[u8]) {
    if buf.len() < REP_HDR {
        return;
    }
    let typ = buf[0];
    let conv = u16::from_le_bytes([buf[1], buf[2]]);
    let _status = i32::from_le_bytes([buf[3], buf[4], buf[5], buf[6]]);
    let plen = u16::from_le_bytes([buf[7], buf[8]]) as usize;
    if REP_HDR + plen > buf.len() {
        return;
    }
    let payload = &buf[REP_HDR..REP_HDR + plen];
    let Some(c) = conv_mut(conv) else {
        return;
    };
    match typ {
        REP_CLONE_OK => {
            if c.status_len == 0 {
                set_status(c, b"cloned");
            }
        }
        REP_DATA => append_data(c, payload),
        REP_STATUS => {
            if payload.is_empty() {
                set_status(c, b"connected");
            } else {
                set_status(c, payload);
            }
        }
        REP_ERR => {
            if payload.is_empty() {
                set_status(c, b"error");
            } else {
                set_status(c, payload);
            }
        }
        _ => {}
    }
}

fn copy_at(src: &[u8], pos: usize, dst: &mut [u8]) -> i32 {
    if pos >= src.len() {
        return 0;
    }
    let n = dst.len().min(src.len() - pos);
    if n != 0 {
        dst[..n].copy_from_slice(&src[pos..pos + n]);
    }
    n as i32
}

unsafe fn c_str<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    if len != 0 && ptr.is_null() {
        return None;
    }
    core::str::from_utf8(unsafe { core::slice::from_raw_parts(ptr, len) }).ok()
}

unsafe fn c_buf_mut<'a>(ptr: *mut u8, len: usize) -> Option<&'a mut [u8]> {
    if len == 0 {
        return Some(unsafe {
            core::slice::from_raw_parts_mut(core::ptr::NonNull::<u8>::dangling().as_ptr(), 0)
        });
    }
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts_mut(ptr, len) })
}

unsafe fn c_buf<'a>(ptr: *const u8, len: usize) -> Option<&'a [u8]> {
    if len == 0 {
        return Some(&[]);
    }
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts(ptr, len) })
}

unsafe extern "C" fn net_lookup(
    _path: *const u8,
    _path_len: usize,
    _out_data: *mut *const u8,
    _out_len: *mut usize,
) -> i32 {
    -1
}

unsafe extern "C" fn net_stat(path: *const u8, path_len: usize, out: *mut VfsStatInfo) -> i32 {
    if out.is_null() {
        return -1;
    }
    let Some(path) = (unsafe { c_str(path, path_len) }) else {
        return -1;
    };
    let Some(node) = parse_path(path) else {
        return -1;
    };
    let (mode, size, ino) = match node {
        Node::Root => (S_IFDIR | 0o755, 0u32, 1u32),
        Node::Proto(p) => (S_IFDIR | 0o755, 0, 10 + p as u32),
        Node::Clone(p) => (S_IFREG | 0o666, 0, 20 + p as u32),
        Node::ConvDir(p, id) => {
            if !conv_ok(id, p) {
                return -1;
            }
            (S_IFDIR | 0o755, 0, 100 + id as u32)
        }
        Node::Ctl(p, id) | Node::Data(p, id) | Node::Status(p, id) => {
            if !conv_ok(id, p) {
                return -1;
            }
            let size = match node {
                Node::Status(_, _) => state().convs[id as usize].status_len as u32,
                Node::Data(_, _) => state().convs[id as usize].data_len as u32,
                _ => 0,
            };
            let tag = match node {
                Node::Ctl(_, _) => 1,
                Node::Data(_, _) => 2,
                _ => 3,
            };
            (S_IFREG | 0o666, size, 200 + (id as u32) * 4 + tag)
        }
    };
    unsafe {
        (*out).mode = mode;
        (*out).size = size;
        (*out).ino = ino;
        (*out).nlink = if mode & S_IFDIR != 0 { 2 } else { 1 };
    }
    0
}

unsafe extern "C" fn net_listdir(
    path: *const u8,
    path_len: usize,
    buf: *mut u8,
    buf_len: usize,
    out_len: *mut usize,
) -> i32 {
    if buf.is_null() || out_len.is_null() {
        return -1;
    }
    let Some(rel) = (unsafe { c_str(path, path_len) }) else {
        return -1;
    };
    let Some(dst) = (unsafe { c_buf_mut(buf, buf_len) }) else {
        return -1;
    };
    let Some(node) = parse_path(rel) else {
        return -1;
    };
    let mut n = 0usize;
    match node {
        Node::Root => {
            let _ = put_bytes(dst, &mut n, b"tcp");
            let _ = put_bytes(dst, &mut n, b"udp");
            let _ = put_bytes(dst, &mut n, b"icmp");
        }
        Node::Proto(p) => {
            let _ = put_bytes(dst, &mut n, b"clone");
            let st = state();
            for i in 0..MAX_CONV {
                if st.convs[i].used && st.convs[i].proto == p {
                    let _ = put_dec(dst, &mut n, i as u16);
                }
            }
        }
        Node::ConvDir(p, id) => {
            if !conv_ok(id, p) {
                return -1;
            }
            let _ = put_bytes(dst, &mut n, b"ctl");
            let _ = put_bytes(dst, &mut n, b"data");
            let _ = put_bytes(dst, &mut n, b"status");
        }
        _ => return -1,
    }
    let _ = proto_name;
    unsafe { *out_len = n };
    0
}

unsafe extern "C" fn net_read(
    path: *const u8,
    path_len: usize,
    pos: usize,
    buf: *mut u8,
    buf_len: usize,
) -> i32 {
    let Some(path) = (unsafe { c_str(path, path_len) }) else {
        return -1;
    };
    let Some(out) = (unsafe { c_buf_mut(buf, buf_len) }) else {
        return -1;
    };
    let Some(node) = parse_path(path) else {
        return -1;
    };
    match node {
        Node::Clone(p) => {
            if pos > 0 {
                return 0;
            }
            let Some(id) = alloc_conv(p) else {
                return 0;
            };
            let _ = enqueue_req(REQ_CLONE, id, p, &[]);
            let mut tmp = [0u8; 8];
            let mut n = 0usize;
            let mut x = id;
            let mut d = [0u8; 6];
            let mut i = d.len();
            if x == 0 {
                i -= 1;
                d[i] = b'0';
            } else {
                while x != 0 && i > 0 {
                    i -= 1;
                    d[i] = b'0' + (x % 10) as u8;
                    x /= 10;
                }
            }
            let digits = &d[i..];
            tmp[..digits.len()].copy_from_slice(digits);
            n = digits.len();
            tmp[n] = b'\n';
            n += 1;
            copy_at(&tmp[..n], 0, out)
        }
        Node::Data(p, id) => {
            let Some(c) = conv_mut(id) else {
                return -1;
            };
            if c.proto != p {
                return -1;
            }
            // Stream: ignore pos. Poll: 0 when empty (do not hang).
            let have = c.data_len as usize;
            if have == 0 {
                return 0;
            }
            let n = out.len().min(have);
            out[..n].copy_from_slice(&c.data[..n]);
            if n < have {
                c.data.copy_within(n..have, 0);
            }
            c.data_len = (have - n) as u16;
            n as i32
        }
        Node::Status(p, id) => {
            let Some(c) = conv_mut(id) else {
                return -1;
            };
            if c.proto != p {
                return -1;
            }
            copy_at(&c.status[..c.status_len as usize], pos, out)
        }
        Node::Ctl(p, id) => {
            let Some(c) = conv_mut(id) else {
                return -1;
            };
            if c.proto != p {
                return -1;
            }
            0
        }
        _ => -1,
    }
}

fn trim_ctl(buf: &[u8]) -> &[u8] {
    let mut s = buf;
    while let Some((&b, rest)) = s.split_first() {
        if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
            s = rest;
        } else {
            break;
        }
    }
    while let Some((&b, rest)) = s.split_last() {
        if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
            s = rest;
        } else {
            break;
        }
    }
    s
}

unsafe extern "C" fn net_write(
    path: *const u8,
    path_len: usize,
    _pos: usize,
    buf: *const u8,
    buf_len: usize,
) -> i32 {
    let Some(path) = (unsafe { c_str(path, path_len) }) else {
        return -1;
    };
    let Some(src) = (unsafe { c_buf(buf, buf_len) }) else {
        return -1;
    };
    let Some(node) = parse_path(path) else {
        return -1;
    };
    match node {
        Node::Ctl(p, id) => {
            if !conv_ok(id, p) {
                return -1;
            }
            let cmd = trim_ctl(src);
            if cmd == b"hangup" {
                set_status(&mut state().convs[id as usize], b"hangup");
                if !enqueue_req(REQ_CLOSE, id, p, &[]) {
                    return -1;
                }
                state().convs[id as usize].used = false;
            } else if !enqueue_req(REQ_CTL, id, p, cmd) {
                return -1;
            }
            src.len() as i32
        }
        Node::Data(p, id) => {
            if !conv_ok(id, p) {
                return -1;
            }
            if src.len() > MSG_CAP - REQ_HDR {
                return -1;
            }
            if !enqueue_req(REQ_SEND, id, p, src) {
                return -1;
            }
            src.len() as i32
        }
        _ => -1,
    }
}

unsafe extern "C" fn chr_read(buf: *mut u8, buf_len: usize) -> i32 {
    if buf.is_null() {
        return -1;
    }
    let out = unsafe { core::slice::from_raw_parts_mut(buf, buf_len) };
    state().req.pop(out) as i32
}

unsafe extern "C" fn chr_write(buf: *const u8, buf_len: usize) -> i32 {
    if buf_len == 0 {
        return 0;
    }
    if buf.is_null() {
        return -1;
    }
    let src = unsafe { core::slice::from_raw_parts(buf, buf_len) };
    apply_reply(src);
    buf_len as i32
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_init(api: *const KernelApi) -> i32 {
    if api.is_null() {
        return -1;
    }
    let api = unsafe { &*api };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    // Build ops here (not in a static): AArch64 ET_EXEC modules do not relocate
    // fn pointers in .rodata, so kernel callbacks need slide-correct addresses.
    let ops = ModuleVfsOps {
        lookup: net_lookup,
        stat: net_stat,
        listdir: net_listdir,
        register: None,
        read: Some(net_read),
        write: Some(net_write),
        create: None,
        truncate: None,
        mkdir: None,
        rmdir: None,
        unlink: None,
        rename: None,
        symlink: None,
        readlink: None,
    };
    let mount_rc = unsafe {
        (api.vfs_mount)(
            b"netfs".as_ptr(),
            5,
            b"net".as_ptr(),
            3,
            &ops as *const ModuleVfsOps,
        )
    };
    let chr = ModuleChrOps {
        read: chr_read,
        write: chr_write,
        ioctl: None,
    };
    let chr_rc = unsafe { (api.dev_register)(b"netd".as_ptr(), 4, &chr) };
    // Stay loaded even if one hook fails: InitFailed would free the image while
    // the other hook still points at it.
    if mount_rc == 0 && chr_rc == 0 {
        unsafe { status_ok(api, "netfs") };
    }
    0
}

#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn module_exit() {}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {}
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
