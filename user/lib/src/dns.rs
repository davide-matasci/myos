//! Shared DNS A-record resolution over Plan 9 `/net/udp`.
//!
//! Used by `/bin/custom/dns` and `/bin/custom/http` — do not duplicate the
//! packet encoder/parser elsewhere.

use crate::{close, open, open_flags, read, write_fd, O_RDWR, O_WRONLY};

const STATUS_POLLS: usize = 400_000;
const DATA_POLLS: usize = 400_000;
const BUF: usize = 1024;

fn put_dec(buf: &mut [u8], mut n: u16) -> usize {
    let mut tmp = [0u8; 6];
    let mut i = tmp.len();
    if n == 0 {
        i -= 1;
        tmp[i] = b'0';
    } else {
        while n != 0 && i > 0 {
            i -= 1;
            tmp[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
    }
    let digits = &tmp[i..];
    buf[..digits.len()].copy_from_slice(digits);
    digits.len()
}

fn conv_path(out: &mut [u8], id: u16, leaf: &[u8]) -> usize {
    let prefix = b"/net/udp/";
    let mut n = 0usize;
    out[n..n + prefix.len()].copy_from_slice(prefix);
    n += prefix.len();
    n += put_dec(&mut out[n..], id);
    out[n] = b'/';
    n += 1;
    out[n..n + leaf.len()].copy_from_slice(leaf);
    n += leaf.len();
    n
}

fn buf_has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn connect_ctl(ip: &[u8], port: u16, out: &mut [u8]) -> usize {
    let prefix = b"connect ";
    let mut n = prefix.len();
    out[..n].copy_from_slice(prefix);
    out[n..n + ip.len()].copy_from_slice(ip);
    n += ip.len();
    out[n] = b'!';
    n += 1;
    n += put_dec(&mut out[n..], port);
    n
}

/// Encode a DNS name (labels). Returns encoded length including the root NUL, or 0 on error.
pub fn encode_name(dst: &mut [u8], name: &[u8]) -> usize {
    let mut pos = 0usize;
    let mut start = 0usize;
    while start < name.len() {
        let mut end = start;
        while end < name.len() && name[end] != b'.' {
            end += 1;
        }
        let len = end - start;
        if len == 0 || len > 63 || pos + 1 + len >= dst.len() {
            return 0;
        }
        dst[pos] = len as u8;
        pos += 1;
        dst[pos..pos + len].copy_from_slice(&name[start..end]);
        pos += len;
        if end == name.len() {
            break;
        }
        start = end + 1;
    }
    if pos >= dst.len() {
        return 0;
    }
    dst[pos] = 0;
    pos + 1
}

/// Parse the first A record from a DNS response.
pub fn parse_a_record(buf: &[u8]) -> Option<[u8; 4]> {
    const HEADER_LEN: usize = 12;
    if buf.len() < HEADER_LEN + 16 {
        return None;
    }
    let mut pos = HEADER_LEN;
    let question_end = loop {
        if pos >= buf.len() {
            return None;
        }
        if (buf[pos] & 0xC0) == 0xC0 {
            pos += 2;
            break pos;
        }
        let label_len = buf[pos] as usize;
        if pos + 1 + label_len > buf.len() {
            return None;
        }
        pos += 1 + label_len;
        if pos < buf.len() && buf[pos] == 0 {
            pos += 1;
            break pos;
        }
    };
    if question_end + 4 > buf.len() {
        return None;
    }
    pos = question_end + 4;
    if pos >= buf.len() {
        return None;
    }
    if (buf[pos] & 0xC0) == 0xC0 {
        pos += 2;
    } else {
        while pos < buf.len() && buf[pos] != 0 {
            let label_len = buf[pos] as usize;
            if pos + 1 + label_len > buf.len() {
                return None;
            }
            pos += 1 + label_len;
        }
        if pos < buf.len() {
            pos += 1;
        }
    }
    if pos + 10 > buf.len() {
        return None;
    }
    // type(2) class(2) ttl(4) rdlength(2)
    let rtype = ((buf[pos] as u16) << 8) | buf[pos + 1] as u16;
    pos += 8;
    let rdlen = ((buf[pos] as usize) << 8) | (buf[pos + 1] as usize);
    pos += 2;
    if rtype == 1 && rdlen == 4 && pos + 4 <= buf.len() {
        return Some([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
    }
    None
}

/// Format dotted IPv4 into `out`. Returns length.
pub fn format_ipv4(ip: [u8; 4], out: &mut [u8]) -> usize {
    let mut pos = 0usize;
    for (i, octet) in ip.iter().enumerate() {
        let mut tmp = [0u8; 4];
        let mut n = *octet as u16;
        let mut j = tmp.len();
        if n == 0 {
            j -= 1;
            tmp[j] = b'0';
        } else {
            while n != 0 && j > 0 {
                j -= 1;
                tmp[j] = b'0' + (n % 10) as u8;
                n /= 10;
            }
        }
        let digits = &tmp[j..];
        if pos + digits.len() + 1 > out.len() {
            break;
        }
        out[pos..pos + digits.len()].copy_from_slice(digits);
        pos += digits.len();
        if i < 3 {
            out[pos] = b'.';
            pos += 1;
        }
    }
    pos
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveError {
    Open,
    Clone,
    Connect,
    Timeout,
    Write,
    NoResponse,
    NoARecord,
    Name,
}

/// Resolve `host` to an IPv4 A record via QEMU DNS (`10.0.2.3:53`).
pub fn resolve_a(host: &[u8]) -> Result<[u8; 4], ResolveError> {
    let dns_server = b"10.0.2.3";
    let dns_port: u16 = 53;

    let mut query = [0u8; 512];
    let mut qpos = 0usize;
    query[qpos] = 0x12;
    query[qpos + 1] = 0x34;
    qpos += 2;
    query[qpos] = 0x01;
    query[qpos + 1] = 0x00;
    qpos += 2;
    query[qpos] = 0x00;
    query[qpos + 1] = 0x01;
    qpos += 2;
    qpos += 6;
    let name_len = encode_name(&mut query[qpos..], host);
    if name_len == 0 {
        return Err(ResolveError::Name);
    }
    qpos += name_len;
    query[qpos] = 0x00;
    query[qpos + 1] = 0x01;
    qpos += 2;
    query[qpos] = 0x00;
    query[qpos + 1] = 0x01;
    qpos += 2;

    let Some(clone) = open(b"/net/udp/clone") else {
        return Err(ResolveError::Open);
    };
    let mut idbuf = [0u8; 16];
    let n = read(clone, &mut idbuf);
    close(clone);
    if n == 0 || n == usize::MAX {
        return Err(ResolveError::Clone);
    }
    let mut id: u16 = 0;
    let mut any = false;
    for &b in &idbuf[..n] {
        match b {
            b'\n' | b'\r' | b' ' => break,
            _ => {
                if !b.is_ascii_digit() {
                    return Err(ResolveError::Clone);
                }
                any = true;
                id = id
                    .checked_mul(10)
                    .and_then(|x| x.checked_add((b - b'0') as u16))
                    .ok_or(ResolveError::Clone)?;
            }
        }
    }
    if !any {
        return Err(ResolveError::Clone);
    }

    let mut pbuf = [0u8; 40];
    let pn = conv_path(&mut pbuf, id, b"ctl");
    let Some(ctl) = open_flags(&pbuf[..pn], O_WRONLY) else {
        return Err(ResolveError::Open);
    };
    let mut cmd = [0u8; 40];
    let cm = connect_ctl(dns_server, dns_port, &mut cmd);
    if write_fd(ctl, &cmd[..cm]) == usize::MAX {
        close(ctl);
        return Err(ResolveError::Write);
    }
    // Keep ctl open until connected so netd sees a live conversation (and so
    // a short-lived write is less likely to race a starved netd). Close after.
    let sn = conv_path(&mut pbuf, id, b"status");
    let mut connected = false;
    let mut last = [0u8; 64];
    let mut last_n = 0usize;
    // "connected" on the stack — avoid relying on rodata string merging for the needle.
    let want = *b"connected";
    for _ in 0..STATUS_POLLS {
        let Some(st) = open(&pbuf[..sn]) else {
            continue;
        };
        let mut sbuf = [0u8; 64];
        let nr = read(st, &mut sbuf);
        close(st);
        if nr > 0 && nr <= 64 {
            last_n = nr;
            last[..nr].copy_from_slice(&sbuf[..nr]);
            if buf_has(&sbuf[..nr], &want) {
                connected = true;
                break;
            }
        }
    }
    close(ctl);
    if !connected {
        // Best-effort hangup so the conv slot is freed for later HTTPS DNS.
        let pn = conv_path(&mut pbuf, id, b"ctl");
        if let Some(h) = open_flags(&pbuf[..pn], O_WRONLY) {
            let _ = write_fd(h, b"hangup");
            close(h);
        }
        // Diagnose: print last status so CI logs show cloned/bad ctl/empty/etc.
        crate::write(b"dns status: ");
        if last_n == 0 {
            crate::write(b"<empty>");
        } else {
            crate::write(&last[..last_n]);
        }
        crate::write(b"\n");
        return Err(ResolveError::Timeout);
    }

    let dn = conv_path(&mut pbuf, id, b"data");
    let Some(data) = open_flags(&pbuf[..dn], O_RDWR) else {
        return Err(ResolveError::Open);
    };
    if write_fd(data, &query[..qpos]) == usize::MAX {
        close(data);
        return Err(ResolveError::Write);
    }

    let mut rbuf = [0u8; BUF];
    for _ in 0..DATA_POLLS {
        let nr = read(data, &mut rbuf);
        if nr == usize::MAX {
            break;
        }
        if nr == 0 {
            continue;
        }
        if let Some(ip) = parse_a_record(&rbuf[..nr]) {
            close(data);
            return Ok(ip);
        }
        // Keep polling — may get NXDOMAIN / CNAME-only first.
    }
    close(data);
    Err(ResolveError::NoARecord)
}
