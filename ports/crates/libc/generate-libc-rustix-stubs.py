#!/usr/bin/env python3
"""Generate ports/crates/libc/rustix_compat.rs from upstream libc Linux defs."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "ports/crates/libc/rustix_compat.rs"
REGISTRY = Path("/usr/local/cargo/registry/src")

# Symbols rustix 1.1.4 needs beyond the minimal myos.rs surface (compile-time only).
NEEDED = """
accept accept4 AF_APPLETALK AF_ASH AF_ATMPVC AF_ATMSVC AF_AX25 AF_BLUETOOTH AF_BRIDGE
AF_CAN AF_DECnet AF_ECONET AF_IEEE802154 AF_INET AF_INET6 AF_IPX AF_IRDA AF_ISDN AF_IUCV
AF_KEY AF_LLC AF_NETBEUI AF_NETLINK AF_NETROM AF_PACKET AF_PHONET AF_PPPOX AF_RDS AF_ROSE
AF_RXRPC AF_SECURITY AF_SNA AF_TIPC AF_UNIX AF_UNSPEC AF_WANPIPE AF_X25 AT_EACCESS
AT_REMOVEDIR AT_SYMLINK_FOLLOW AT_SYMLINK_NOFOLLOW bind chroot clock_getres
CLOCK_PROCESS_CPUTIME_ID clock_settime CLOCK_THREAD_CPUTIME_ID closedir connect dirfd
dlsym dup3 E2BIG EADDRINUSE EADDRNOTAVAIL EADV EAFNOSUPPORT EAGAIN EALREADY EBADE EBADFD
EBADMSG EBADR EBADRQC EBADSLT EBFONT EBUSY ECANCELED ECHRNG ECOMM ECONNABORTED ECONNREFUSED
ECONNRESET EDEADLK EDEADLOCK EDESTADDRREQ EDOM EDOTDOT EDQUOT EFBIG EHOSTDOWN EHOSTUNREACH
EHWPOISON EIDRM EILSEQ EINPROGRESS EISCONN EISDIR EISNAM EKEYEXPIRED EKEYREJECTED EKEYREVOKED
EL2HLT EL2NSYNC EL3HLT EL3RST ELIBACC ELIBBAD ELIBEXEC ELIBMAX ELIBSCN ELNRNG EMEDIUMTYPE
EMFILE EMLINK EMSGSIZE EMULTIHOP ENAVAIL ENETDOWN ENETRESET ENETUNREACH ENFILE ENOANO ENOBUFS
ENOCSI ENODATA ENODEV ENOEXEC ENOKEY ENOLCK ENOLINK ENOMEDIUM ENOMSG ENONET ENOPKG
ENOPROTOOPT ENOSPC ENOSR ENOSTR ENOTBLK ENOTCONN ENOTNAM ENOTRECOVERABLE ENOTSOCK ENOTSUP
ENOTTY ENOTUNIQ ENXIO EOPNOTSUPP EOWNERDEAD EPFNOSUPPORT EPIPE EPROTO EPROTONOSUPPORT
EPROTOTYPE ERANGE EREMCHG EREMOTE EREMOTEIO ERESTART ERFKILL EROFS ESHUTDOWN ESOCKTNOSUPPORT
ESPIPE ESRCH ESRMNT ESTALE ESTRPIPE ETIME ETIMEDOUT ETOOMANYREFS ETXTBSY EUCLEAN EUNATCH
EUSERS EWOULDBLOCK EXDEV EXFULL FALLOC_FL_COLLAPSE_RANGE FALLOC_FL_INSERT_RANGE
FALLOC_FL_KEEP_SIZE FALLOC_FL_NO_HIDE_STALE FALLOC_FL_PUNCH_HOLE FALLOC_FL_UNSHARE_RANGE
FALLOC_FL_ZERO_RANGE fchdir fchmodat fchownat fdopendir F_DUPFD_CLOEXEC F_GETLK FIONBIO
FIONREAD F_OK F_RDLCK fstatfs fstatvfs F_UNLCK futimens F_WRLCK getgroups getpeername getpgid
getpgrp getppid getpriority getrlimit getsid getsockname getsockopt IP_ADD_MEMBERSHIP
IP_DROP_MEMBERSHIP IP_MULTICAST_IF IP_MULTICAST_LOOP IP_MULTICAST_TTL IPPROTO_AH
IPPROTO_BEETPH IPPROTO_COMP IPPROTO_DCCP IPPROTO_EGP IPPROTO_ENCAP IPPROTO_ESP
IPPROTO_FRAGMENT IPPROTO_GRE IPPROTO_ICMP IPPROTO_ICMPV6 IPPROTO_IDP IPPROTO_IGMP IPPROTO_IP
IPPROTO_IPIP IPPROTO_IPV6 IPPROTO_MH IPPROTO_MPLS IPPROTO_MPTCP IPPROTO_MTP IPPROTO_PIM
IPPROTO_PUP IPPROTO_RAW IPPROTO_ROUTING IPPROTO_RSVP IPPROTO_SCTP IPPROTO_TCP IPPROTO_TP
IPPROTO_UDP IPPROTO_UDPLITE IP_TTL IPV6_MULTICAST_HOPS IPV6_MULTICAST_IF IPV6_MULTICAST_LOOP
IPV6_TCLASS IPV6_UNICAST_HOPS IPV6_V6ONLY listen LOCK_EX LOCK_NB LOCK_SH LOCK_UN major
makedev minor mknodat MSG_CMSG_CLOEXEC MSG_CONFIRM MSG_CTRUNC MSG_DONTROUTE MSG_DONTWAIT
MSG_EOR MSG_ERRQUEUE MSG_MORE MSG_NOSIGNAL MSG_OOB MSG_PEEK MSG_TRUNC MSG_WAITALL nice
O_ACCMODE O_ASYNC O_DIRECT O_DSYNC O_NOCTTY O_SYNC P_ALL PIPE_BUF POSIX_FADV_DONTNEED
posix_fadvise POSIX_FADV_NOREUSE POSIX_FADV_NORMAL POSIX_FADV_RANDOM POSIX_FADV_SEQUENTIAL
POSIX_FADV_WILLNEED posix_fallocate P_PGID P_PID preadv PRIO_PGRP PRIO_PROCESS PRIO_USER
pwritev recv recvfrom recvmsg rewinddir RLIM_INFINITY RLIMIT_AS RLIMIT_CORE RLIMIT_CPU
RLIMIT_DATA RLIMIT_FSIZE RLIMIT_LOCKS RLIMIT_MEMLOCK RLIMIT_MSGQUEUE RLIMIT_NICE RLIMIT_NOFILE
RLIMIT_NPROC RLIMIT_RSS RLIMIT_RTPRIO RLIMIT_RTTIME RLIMIT_SIGPENDING RLIMIT_STACK R_OK
_SC_CLK_TCK SCM_RIGHTS _SC_PAGESIZE SEEK_CUR seekdir SEEK_END SEEK_SET send sendmsg sendto
setpgid setpriority setrlimit setsid setsockopt shutdown SHUT_RD SHUT_RDWR SHUT_WR S_IFBLK
S_IFCHR S_IFIFO S_IFSOCK SIGABRT SIGALRM SIGBUS SIGCHLD SIGCONT SIGFPE SIGHUP SIGILL SIGINT
SIGIO SIGKILL SIGPIPE SIGPROF SIGPWR SIGQUIT SIGSEGV SIGSTKFLT SIGSTOP SIGSYS SIGTERM SIGTRAP
SIGTSTP SIGTTIN SIGTTOU SIGURG SIGUSR1 SIGUSR2 SIGVTALRM SIGWINCH SIGXCPU SIGXFSZ S_IRGRP
S_IROTH S_IRUSR S_IRWXG S_IRWXO S_IRWXU S_ISGID S_ISUID S_ISVTX S_IWGRP S_IWOTH S_IWUSR
S_IXGRP S_IXOTH S_IXUSR SO_ACCEPTCONN SO_BROADCAST SOCK_CLOEXEC SOCK_DGRAM socket socketpair
SOCK_NONBLOCK SOCK_RAW SOCK_RDM SOCK_SEQPACKET SOCK_STREAM SO_DOMAIN SO_ERROR SO_KEEPALIVE
SO_LINGER SOL_SOCKET SO_OOBINLINE SO_RCVBUF SO_RCVTIMEO SO_REUSEADDR SO_REUSEPORT SO_SNDBUF
SO_SNDTIMEO SO_TYPE statfs statvfs ST_NOSUID ST_RDONLY sync sysconf TCP_KEEPCNT TCP_KEEPINTVL
TCP_NODELAY TIOCSCTTY umask waitid W_OK X_OK CLD_CONTINUED CLD_DUMPED CLD_EXITED CLD_KILLED
CLD_STOPPED CLD_TRAPPED EXIT_FAILURE EXIT_SUCCESS RTLD_DEFAULT UTIME_NOW UTIME_OMIT DT_BLK
IPV6_ADD_MEMBERSHIP IPV6_DROP_MEMBERSHIP TCP_KEEPIDLE F_SETLK F_SETLKW
WCONTINUED WUNTRACED WEXITED WNOWAIT WSTOPPED WNOHANG _SC_CLK_TCK _SC_PAGESIZE
readdir flock
""".split()

SKIP = {
    # Already defined in myos.rs
    "EPERM", "ENOENT", "EINTR", "EIO", "EBADF", "ENOMEM", "EACCES", "EFAULT", "EEXIST",
    "ENOTDIR", "EINVAL", "ENOSYS", "ENOTEMPTY", "ELOOP", "ENAMETOOLONG", "EOVERFLOW", "ECHILD",
    "O_RDONLY", "O_WRONLY", "O_RDWR", "O_CREAT", "O_EXCL", "O_TRUNC", "O_APPEND", "O_CLOEXEC",
    "O_DIRECTORY", "O_NOFOLLOW", "O_NONBLOCK", "S_IFMT", "S_IFREG", "S_IFDIR", "S_IFLNK",
    "DT_UNKNOWN", "DT_REG", "DT_DIR", "DT_LNK", "CLOCK_REALTIME", "CLOCK_MONOTONIC",
    "F_DUPFD", "F_GETFD", "F_SETFD", "F_GETFL", "F_SETFL", "FD_CLOEXEC", "AT_FDCWD",
    # Types / macros handled in the types block below
    "cmsghdr", "msghdr", "siginfo_t", "rlimit", "fsid_t", "in_addr", "in6_addr",
    "sockaddr", "sockaddr_in", "sockaddr_in6", "sockaddr_storage", "linger", "ip_mreq",
    "ipv6_mreq", "sockaddr_un", "sync", "RTLD_DEFAULT", "AF_UNIX", "AF_LOCAL",
    "RLIMIT_AS", "RLIMIT_CORE", "RLIMIT_CPU", "RLIMIT_DATA", "RLIMIT_FSIZE",
    "RLIMIT_LOCKS", "RLIMIT_MEMLOCK", "RLIMIT_MSGQUEUE", "RLIMIT_NICE",
    "RLIMIT_NOFILE", "RLIMIT_NPROC", "RLIMIT_RSS", "RLIMIT_RTPRIO",
    "RLIMIT_RTTIME", "RLIMIT_SIGPENDING", "RLIMIT_STACK",
}

MANUAL_STUBS = """
pub unsafe fn sync() {}
pub unsafe fn dlsym(_handle: *mut c_void, _symbol: *const c_char) -> *mut c_void {
    syscalls::set_errno(ENOSYS);
    core::ptr::null_mut()
}
pub unsafe fn fdopendir(_fd: c_int) -> *mut DIR {
    syscalls::set_errno(ENOSYS);
    core::ptr::null_mut()
}
pub unsafe fn readdir(_dirp: *mut DIR) -> *mut dirent {
    syscalls::set_errno(ENOSYS);
    core::ptr::null_mut()
}
pub unsafe fn rewinddir(_dirp: *mut DIR) {}
pub unsafe fn seekdir(_dirp: *mut DIR, _loc: c_long) {
    let _ = _dirp;
}

safe_f! {
    pub const safe fn major(dev: dev_t) -> c_uint {
        ((dev >> 8) & 0xfff) as c_uint | ((dev >> 32) & !0xfff) as c_uint
    }
    pub const safe fn minor(dev: dev_t) -> c_uint {
        (dev & 0xff) as c_uint | ((dev >> 12) & !0xff) as c_uint
    }
    pub const safe fn makedev(major: c_uint, minor: c_uint) -> dev_t {
        ((major & 0xfff) as dev_t) << 8
            | ((major & !0xfff) as dev_t) << 32
            | ((minor & 0xff) as dev_t)
            | ((minor & !0xff) as dev_t) << 12
    }

    pub const safe fn WIFSTOPPED(status: c_int) -> bool {
        (status & 0xff) == 0x7f
    }
    pub const safe fn WSTOPSIG(status: c_int) -> c_int {
        (status >> 8) & 0xff
    }
    pub const safe fn WIFCONTINUED(status: c_int) -> bool {
        status == 0xffff
    }
    pub const safe fn WIFSIGNALED(status: c_int) -> bool {
        ((status & 0x7f) + 1) as i8 >= 2
    }
    pub const safe fn WTERMSIG(status: c_int) -> c_int {
        status & 0x7f
    }
    pub const safe fn WIFEXITED(status: c_int) -> bool {
        (status & 0x7f) == 0
    }
    pub const safe fn WEXITSTATUS(status: c_int) -> c_int {
        (status >> 8) & 0xff
    }
}
"""

FUNCTIONS = {
    name
    for name in NEEDED
    if name[0].islower() and name not in SKIP and name not in {
        "sync", "dlsym", "fdopendir", "readdir", "rewinddir", "seekdir",
        "major", "minor", "makedev",
    }
}

CONSTANTS = {
    name
    for name in NEEDED
    if (name[0].isupper() or name.startswith("_")) and name not in SKIP
}


def find_libc_root() -> Path:
    for base in REGISTRY.glob("index.crates.io-*/libc-0.2.*"):
        if (base / "src/unix/linux_like/mod.rs").is_file():
            return base
    raise SystemExit("libc crate not found in cargo registry")


def scan_constants(root: Path) -> dict[str, str]:
    pat = re.compile(r"^pub const (\w+): ([^=]+) = (.+);$")
    found: dict[str, str] = {}
    for path in (root / "src/unix").rglob("*.rs"):
        try:
            text = path.read_text()
        except OSError:
            continue
        for line in text.splitlines():
            m = pat.match(line.strip())
            if not m:
                continue
            name, ty, val = m.group(1), m.group(2).strip(), m.group(3).strip()
            if name in CONSTANTS and name not in found:
                found[name] = f"pub const {name}: {ty} = {val};"
    return found


TYPES_AND_MACROS = """
pub type in_addr_t = u32;
pub type in_port_t = u16;
pub type rlim_t = u64;
pub type idtype_t = c_uint;
pub type __rlimit_resource_t = c_uint;

s! {
    pub struct msghdr {
        pub msg_name: *mut c_void,
        pub msg_namelen: socklen_t,
        pub msg_iov: *mut iovec,
        pub msg_iovlen: c_int,
        pub msg_control: *mut c_void,
        pub msg_controllen: size_t,
        pub msg_flags: c_int,
    }

    pub struct cmsghdr {
        pub cmsg_len: size_t,
        pub cmsg_level: c_int,
        pub cmsg_type: c_int,
    }

    pub struct siginfo_t {
        pub si_signo: c_int,
        pub si_errno: c_int,
        pub si_code: c_int,
        pub _pad: [c_int; 29],
    }

    pub struct flock {
        pub l_type: c_short,
        pub l_whence: c_short,
        pub l_start: off_t,
        pub l_len: off_t,
        pub l_pid: pid_t,
    }

    pub struct rlimit {
        pub rlim_cur: rlim_t,
        pub rlim_max: rlim_t,
    }

    pub struct fsid_t {
        __val: [c_int; 2],
    }

    pub struct in_addr {
        pub s_addr: in_addr_t,
    }

    pub struct in6_addr {
        pub s6_addr: [u8; 16],
    }

    pub struct sockaddr {
        pub sa_family: sa_family_t,
        pub sa_data: [c_char; 14],
    }

    pub struct sockaddr_in {
        pub sin_family: sa_family_t,
        pub sin_port: in_port_t,
        pub sin_addr: in_addr,
        pub sin_zero: [u8; 8],
    }

    pub struct sockaddr_in6 {
        pub sin6_family: sa_family_t,
        pub sin6_port: in_port_t,
        pub sin6_flowinfo: u32,
        pub sin6_addr: in6_addr,
        pub sin6_scope_id: u32,
    }

    pub struct sockaddr_storage {
        pub ss_family: sa_family_t,
        pub __data: [u8; 126],
    }

    pub struct linger {
        pub l_onoff: c_int,
        pub l_linger: c_int,
    }

    pub struct ip_mreq {
        pub imr_multiaddr: in_addr,
        pub imr_interface: in_addr,
    }

    pub struct ipv6_mreq {
        pub ipv6mr_multiaddr: in6_addr,
        pub ipv6mr_interface: c_uint,
    }

    pub struct statfs {
        pub f_type: c_long,
        pub f_bsize: c_long,
        pub f_blocks: u64,
        pub f_bfree: u64,
        pub f_bavail: u64,
        pub f_files: u64,
        pub f_ffree: u64,
        pub f_fsid: fsid_t,
        pub f_namelen: c_long,
        pub f_frsize: c_long,
        pub f_spare: [c_long; 5],
    }

    pub struct statvfs {
        pub f_bsize: c_ulong,
        pub f_frsize: c_ulong,
        pub f_blocks: u64,
        pub f_bfree: u64,
        pub f_bavail: u64,
        pub f_files: u64,
        pub f_ffree: u64,
        pub f_favail: u64,
        pub f_fsid: c_ulong,
        pub f_flag: c_ulong,
        pub f_namemax: c_ulong,
        pub f_spare: [c_int; 6],
    }

    pub struct sockaddr_un {
        pub sun_family: sa_family_t,
        pub sun_path: [c_char; 108],
    }
}

impl siginfo_t {
    pub unsafe fn si_status(&self) -> c_int {
        0
    }
}

const fn cmsg_align(len: usize) -> usize {
    (len + core::mem::size_of::<usize>() - 1) & !(core::mem::size_of::<usize>() - 1)
}

f! {
    pub unsafe fn CMSG_FIRSTHDR(mhdr: *const msghdr) -> *mut cmsghdr {
        if (*mhdr).msg_controllen >= core::mem::size_of::<cmsghdr>() {
            (*mhdr).msg_control.cast()
        } else {
            core::ptr::null_mut()
        }
    }

    pub unsafe fn CMSG_DATA(cmsg: *const cmsghdr) -> *mut c_uchar {
        (cmsg as *mut cmsghdr).offset(1).cast()
    }

    pub const unsafe fn CMSG_SPACE(length: c_uint) -> c_uint {
        (cmsg_align(length as usize) + cmsg_align(core::mem::size_of::<cmsghdr>())) as c_uint
    }

    pub const unsafe fn CMSG_LEN(length: c_uint) -> c_uint {
        cmsg_align(core::mem::size_of::<cmsghdr>()) as c_uint + length
    }

    pub unsafe fn CMSG_NXTHDR(mhdr: *const msghdr, cmsg: *const cmsghdr) -> *mut cmsghdr {
        let end = unsafe { (*mhdr).msg_control.cast::<u8>().add((*mhdr).msg_controllen) };
        let next = unsafe {
            cmsg.cast::<u8>()
                .add(cmsg_align((*cmsg).cmsg_len as usize))
                .cast::<cmsghdr>()
        };
        if next.cast::<u8>() >= end {
            core::ptr::null_mut()
        } else {
            next as *mut cmsghdr
        }
    }
}

// RTLD_DEFAULT lives in linux_l4re_shared in upstream libc; omit duplicate if present.
#[allow(unused)]
const _RTLD_DEFAULT_PLACEHOLDER: () = ();
"""


def fn_sig(name: str) -> tuple[str, str]:
    sigs: dict[str, tuple[str, str]] = {
        "accept": ("fd: c_int, addr: *mut sockaddr, addrlen: *mut socklen_t", "c_int"),
        "accept4": ("fd: c_int, addr: *mut sockaddr, addrlen: *mut socklen_t, flags: c_int", "c_int"),
        "bind": ("fd: c_int, addr: *const sockaddr, len: socklen_t", "c_int"),
        "chroot": ("path: *const c_char", "c_int"),
        "clock_getres": ("clk: clockid_t, tp: *mut timespec", "c_int"),
        "clock_settime": ("clk: clockid_t, tp: *const timespec", "c_int"),
        "closedir": ("dirp: *mut DIR", "c_int"),
        "connect": ("fd: c_int, addr: *const sockaddr, len: socklen_t", "c_int"),
        "dirfd": ("dirp: *mut DIR", "c_int"),
        "dlsym": ("handle: *mut c_void, symbol: *const c_char", "*mut c_void"),
        "dup3": ("oldfd: c_int, newfd: c_int, flags: c_int", "c_int"),
        "fchdir": ("fd: c_int", "c_int"),
        "fchmodat": ("dirfd: c_int, path: *const c_char, mode: mode_t, flags: c_int", "c_int"),
        "fchownat": ("dirfd: c_int, path: *const c_char, owner: uid_t, group: gid_t, flags: c_int", "c_int"),
        "fdopendir": ("fd: c_int", "*mut DIR"),
        "fstatfs": ("fd: c_int, buf: *mut statfs", "c_int"),
        "fstatvfs": ("fd: c_int, buf: *mut statvfs", "c_int"),
        "futimens": ("fd: c_int, times: *const timespec", "c_int"),
        "getgroups": ("size: c_int, list: *mut gid_t", "c_int"),
        "getpeername": ("fd: c_int, addr: *mut sockaddr, len: *mut socklen_t", "c_int"),
        "getpgid": ("pid: pid_t", "pid_t"),
        "getpgrp": ("", "pid_t"),
        "getppid": ("", "pid_t"),
        "getpriority": ("which: c_int, who: c_int", "c_int"),
        "getrlimit": ("resource: c_int, rlim: *mut rlimit", "c_int"),
        "getsid": ("pid: pid_t", "pid_t"),
        "getsockname": ("fd: c_int, addr: *mut sockaddr, len: *mut socklen_t", "c_int"),
        "getsockopt": ("fd: c_int, level: c_int, optname: c_int, optval: *mut c_void, optlen: *mut socklen_t", "c_int"),
        "listen": ("fd: c_int, backlog: c_int", "c_int"),
        "major": ("dev: dev_t", "c_uint"),
        "makedev": ("major: c_uint, minor: c_uint", "dev_t"),
        "minor": ("dev: dev_t", "c_uint"),
        "mknodat": ("dirfd: c_int, path: *const c_char, mode: mode_t, dev: dev_t", "c_int"),
        "nice": ("inc: c_int", "c_int"),
        "posix_fadvise": ("fd: c_int, offset: off_t, len: off_t, advice: c_int", "c_int"),
        "posix_fallocate": ("fd: c_int, offset: off_t, len: off_t", "c_int"),
        "preadv": ("fd: c_int, iov: *const iovec, iovcnt: c_int, offset: off_t", "ssize_t"),
        "pwritev": ("fd: c_int, iov: *const iovec, iovcnt: c_int, offset: off_t", "ssize_t"),
        "recv": ("fd: c_int, buf: *mut c_void, len: size_t, flags: c_int", "ssize_t"),
        "recvfrom": ("fd: c_int, buf: *mut c_void, len: size_t, flags: c_int, addr: *mut sockaddr, addrlen: *mut socklen_t", "ssize_t"),
        "recvmsg": ("fd: c_int, msg: *mut msghdr, flags: c_int", "ssize_t"),
        "rewinddir": ("dirp: *mut DIR", "c_void"),
        "seekdir": ("dirp: *mut DIR, loc: c_long", "c_void"),
        "send": ("fd: c_int, buf: *const c_void, len: size_t, flags: c_int", "ssize_t"),
        "sendmsg": ("fd: c_int, msg: *const msghdr, flags: c_int", "ssize_t"),
        "sendto": ("fd: c_int, buf: *const c_void, len: size_t, flags: c_int, addr: *const sockaddr, addrlen: socklen_t", "ssize_t"),
        "setpgid": ("pid: pid_t, pgid: pid_t", "c_int"),
        "setpriority": ("which: c_int, who: c_int, prio: c_int", "c_int"),
        "setrlimit": ("resource: c_int, rlim: *const rlimit", "c_int"),
        "setsid": ("", "pid_t"),
        "setsockopt": ("fd: c_int, level: c_int, optname: c_int, optval: *const c_void, optlen: socklen_t", "c_int"),
        "shutdown": ("fd: c_int, how: c_int", "c_int"),
        "socket": ("domain: c_int, ty: c_int, protocol: c_int", "c_int"),
        "socketpair": ("domain: c_int, ty: c_int, protocol: c_int, sv: *mut c_int", "c_int"),
        "statfs": ("path: *const c_char, buf: *mut statfs", "c_int"),
        "statvfs": ("path: *const c_char, buf: *mut statvfs", "c_int"),
        "sync": ("", "c_void"),
        "sysconf": ("name: c_int", "c_long"),
        "umask": ("mask: mode_t", "mode_t"),
        "waitid": ("idtype: idtype_t, id: c_int, infop: *mut siginfo_t, options: c_int", "c_int"),
        "readdir": ("dirp: *mut DIR", "*mut dirent"),
        "flock": ("fd: c_int, operation: c_int", "c_int"),
        "statfs": ("path: *const c_char, buf: *mut statfs", "c_int"),
        "statvfs": ("path: *const c_char, buf: *mut statvfs", "c_int"),
    }
    args, ret = sigs.get(name, ("_unused: *mut c_void", "c_int"))
    if args:
        return args, ret
    return "", ret


def main() -> int:
    root = find_libc_root()
    found = scan_constants(root)
    missing = sorted(CONSTANTS - set(found))
    if missing:
        print(f"warning: {len(missing)} constants not found in upstream libc, using 0:", file=sys.stderr)
        for name in missing:
            found[name] = f"pub const {name}: c_int = 0;"

    lines = [
        "// Auto-generated by ports/crates/libc/generate-libc-rustix-stubs.py — compile-time rustix groundwork.",
        "// Runtime ENOSYS is fine until individual syscalls matter.",
        "",
        TYPES_AND_MACROS.strip(),
        "",
        "// Linux-compatible constants (from upstream libc where available).",
    ]
    for name in sorted(CONSTANTS):
        lines.append(found[name])
    lines.append("pub const AF_LOCAL: c_int = 1;")
    lines.append("pub const AF_UNIX: c_int = 1;")
    lines.append("pub const RTLD_DEFAULT: *mut c_void = core::ptr::null_mut();")
    lines.append("pub const DT_CHR: c_uchar = 2;")
    lines.append("pub const DT_FIFO: c_uchar = 1;")
    lines.append("pub const DT_SOCK: c_uchar = 12;")
    lines.extend([
        "pub const RLIMIT_CPU: c_int = 0;",
        "pub const RLIMIT_FSIZE: c_int = 1;",
        "pub const RLIMIT_DATA: c_int = 2;",
        "pub const RLIMIT_STACK: c_int = 3;",
        "pub const RLIMIT_CORE: c_int = 4;",
        "pub const RLIMIT_RSS: c_int = 5;",
        "pub const RLIMIT_NPROC: c_int = 6;",
        "pub const RLIMIT_NOFILE: c_int = 7;",
        "pub const RLIMIT_MEMLOCK: c_int = 8;",
        "pub const RLIMIT_AS: c_int = 9;",
        "pub const RLIMIT_LOCKS: c_int = 10;",
        "pub const RLIMIT_SIGPENDING: c_int = 11;",
        "pub const RLIMIT_MSGQUEUE: c_int = 12;",
        "pub const RLIMIT_NICE: c_int = 13;",
        "pub const RLIMIT_RTPRIO: c_int = 14;",
        "pub const RLIMIT_RTTIME: __rlimit_resource_t = 15;",
    ])

    lines.extend(["", MANUAL_STUBS.strip(), "", "// ENOSYS stubs for rustix-referenced libc functions.", "enosys! {"])
    for name in sorted(FUNCTIONS):
        args, ret = fn_sig(name)
        if args:
            lines.append(f"    pub unsafe fn {name}({args}) -> {ret};")
        else:
            lines.append(f"    pub unsafe fn {name}() -> {ret};")
    lines.append("}")

    OUT.write_text("\n".join(lines) + "\n")
    print(f"Wrote {OUT} ({len(lines)} lines, {len(FUNCTIONS)} fn stubs, {len(CONSTANTS)} constants)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
