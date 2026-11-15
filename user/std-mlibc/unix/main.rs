//! The mlibc flavour of the Rust std is a unix target: std's own unix code
//! over the C library. What a compiler or a build tool asks of it, checked
//! here: the environment, arguments of any number and length, child
//! processes with pipes, the file system with `std::os::unix`, sockets,
//! time and threads. `/bin/std/mlibc/unix` runs every check; with
//! `child ...` it is the child of its own process checks.

use std::collections::HashMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt, symlink};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio, exit};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

macro_rules! check {
    ($cond:expr, $($msg:tt)*) => {
        if !$cond {
            eprintln!("FAIL {}:{}: {}", file!(), line!(), format!($($msg)*));
            exit(1);
        }
    };
}

/// The child of the process checks: what it was given, on its stdout.
fn child(args: &[String]) -> ! {
    match args.first().map(String::as_str) {
        // Its arguments, one a line, then their count.
        Some("args") => {
            for a in &args[1..] {
                println!("{}", a.len());
            }
            println!("n={}", args.len() - 1);
        }
        // The variables named, as NAME=value or NAME unset.
        Some("env") => {
            for name in &args[1..] {
                match env::var(name) {
                    Ok(v) => println!("{name}={v}"),
                    Err(_) => println!("{name} unset"),
                }
            }
            println!("count={}", env::vars().count());
        }
        // Its stdin in capitals on stdout, the byte count on stderr.
        Some("upper") => {
            let mut s = String::new();
            io::stdin().read_to_string(&mut s).unwrap();
            print!("{}", s.to_uppercase());
            eprint!("{}", s.len());
        }
        Some("cwd") => println!("{}", env::current_dir().unwrap().display()),
        Some("exit") => exit(args[1].parse().unwrap()),
        Some("sleep") => thread::sleep(Duration::from_secs(60)),
        other => {
            eprintln!("unknown child mode {other:?}");
            exit(2);
        }
    }
    exit(0)
}

fn me() -> PathBuf {
    env::current_exe().unwrap()
}

fn check_args_env() {
    let exe = me();
    check!(exe.is_absolute() && exe.exists(), "current_exe {exe:?}");
    // 40 arguments, one of 3000 bytes: no limit of the runtime's.
    let long = "x".repeat(3000);
    let mut cmd = Command::new(&exe);
    cmd.args(["child", "args"]);
    for i in 0..39 {
        cmd.arg(format!("a{i}"));
    }
    cmd.arg(&long);
    let out = cmd.output().unwrap();
    check!(out.status.success(), "args child: {:?}", out.status);
    let text = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    check!(lines.len() == 41 && lines[40] == "n=40" && lines[39] == "3000", "args child said {text:?}");

    // The environment: set here, inherited, added to and cleared.
    unsafe { env::set_var("STD_UNIX_A", "one") };
    check!(env::var("STD_UNIX_A").as_deref() == Ok("one"), "set_var then var");
    check!(env::vars().any(|(k, v)| k == "STD_UNIX_A" && v == "one"), "vars");
    unsafe { env::remove_var("STD_UNIX_A") };
    check!(env::var_os("STD_UNIX_A").is_none(), "remove_var");
    unsafe { env::set_var("STD_UNIX_B", "two") };
    let out = Command::new(&exe)
        .args(["child", "env", "STD_UNIX_B", "STD_UNIX_C", "STD_UNIX_D"])
        .env("STD_UNIX_C", "three words here")
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    check!(
        text.starts_with("STD_UNIX_B=two\nSTD_UNIX_C=three words here\nSTD_UNIX_D unset\n"),
        "env child said {text:?}"
    );
    let out = Command::new(&exe)
        .args(["child", "env", "STD_UNIX_B"])
        .env_clear()
        .env("ONLY", "1")
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    check!(text == "STD_UNIX_B unset\ncount=1\n", "cleared env child said {text:?}");
}

fn check_process(dir: &Path) {
    let exe = me();
    // Pipes on the three streams.
    let mut kid = Command::new(&exe)
        .args(["child", "upper"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    kid.stdin.take().unwrap().write_all(b"hello pipes\n").unwrap();
    let out = kid.wait_with_output().unwrap();
    check!(out.stdout == b"HELLO PIPES\n" && out.stderr == b"12", "piped child: {out:?}");
    // The exit status, and a directory to start in.
    let st = Command::new(&exe).args(["child", "exit", "7"]).status().unwrap();
    check!(st.code() == Some(7), "exit code {st:?}");
    let out = Command::new(&exe).args(["child", "cwd"]).current_dir(dir).output().unwrap();
    let said = String::from_utf8(out.stdout).unwrap();
    check!(Path::new(said.trim()) == fs::canonicalize(dir).unwrap(), "child's cwd {said:?}");
    // A program found through PATH, its output to a file.
    let log = dir.join("echo.out");
    let st = Command::new("echo").arg("from PATH").stdout(File::create(&log).unwrap()).status().unwrap();
    check!(st.success() && fs::read_to_string(&log).unwrap() == "from PATH\n", "echo through PATH");
    // One that is not there.
    let err = Command::new("/nonexistent/program").spawn().unwrap_err();
    check!(err.kind() == io::ErrorKind::NotFound, "missing program: {err:?}");
    // Killed: the signal is its status.
    let mut kid = Command::new(&exe).args(["child", "sleep"]).spawn().unwrap();
    check!(kid.try_wait().unwrap().is_none(), "the sleeper ended early");
    kid.kill().unwrap();
    let st = kid.wait().unwrap();
    check!(st.signal() == Some(9) && st.code().is_none(), "killed child: {st:?}");
    check!(std::process::id() > 1, "pid");
}

fn check_fs(dir: &Path) {
    let sub = dir.join("a/b/c");
    fs::create_dir_all(&sub).unwrap();
    let file = sub.join("file.txt");
    fs::write(&file, b"0123456789").unwrap();
    let md = fs::metadata(&file).unwrap();
    check!(md.is_file() && md.len() == 10 && md.size() == 10, "metadata {md:?}");
    check!(md.nlink() == 1 && md.ino() != 0, "nlink {} ino {}", md.nlink(), md.ino());
    let age = SystemTime::now().duration_since(md.modified().unwrap()).unwrap();
    check!(age < Duration::from_secs(120), "mtime is {age:?} old");

    // Positions, positional reads, append, truncation.
    let mut f = OpenOptions::new().read(true).write(true).open(&file).unwrap();
    f.seek(SeekFrom::Start(4)).unwrap();
    let mut two = [0u8; 2];
    f.read_exact(&mut two).unwrap();
    check!(&two == b"45", "seek and read");
    f.read_exact_at(&mut two, 8).unwrap();
    check!(&two == b"89" && f.stream_position().unwrap() == 6, "read_at moved the position");
    f.set_len(4).unwrap();
    f.sync_all().unwrap();
    drop(f);
    OpenOptions::new().append(true).open(&file).unwrap().write_all(b"ab").unwrap();
    check!(fs::read(&file).unwrap() == b"0123ab", "set_len and append");

    // Modes: myos keeps none (docs/security.md), so a change is taken
    // and the owner goes on reading and writing.
    fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    let mode = fs::metadata(&file).unwrap().permissions().mode();
    check!(mode & 0o600 == 0o600, "mode {mode:o}");

    // Links.
    let link = sub.join("link");
    symlink("file.txt", &link).unwrap();
    check!(fs::read_link(&link).unwrap() == Path::new("file.txt"), "read_link");
    check!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "symlink_metadata");
    check!(fs::read(&link).unwrap() == b"0123ab", "through the symlink");
    // No hard links on myos, on purpose (docs/fs.md): refused, as cargo
    // and the like expect of a file system without them, and they copy.
    let hard = sub.join("hard");
    let err = fs::hard_link(&file, &hard).unwrap_err();
    check!(err.kind() == io::ErrorKind::PermissionDenied, "hard_link: {err:?}");
    fs::copy(&file, &hard).unwrap();
    let canon = fs::canonicalize(sub.join("../c/./link")).unwrap();
    check!(canon == fs::canonicalize(&file).unwrap(), "canonicalize {canon:?}");

    // The directory, renames, removal.
    fs::rename(&hard, sub.join("renamed")).unwrap();
    let mut names: Vec<String> =
        fs::read_dir(&sub).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    check!(names == ["file.txt", "link", "renamed"], "read_dir {names:?}");
    let kinds: HashMap<_, _> = fs::read_dir(&sub)
        .unwrap()
        .map(|e| e.unwrap())
        .map(|e| (e.file_name(), e.file_type().unwrap().is_symlink()))
        .collect();
    check!(kinds[Path::new("link").as_os_str()], "the entry's type: {kinds:?}");
    check!(fs::copy(&file, sub.join("copy")).unwrap() == 6, "copy");
    let err = fs::remove_dir(&sub).unwrap_err();
    check!(err.kind() == io::ErrorKind::DirectoryNotEmpty, "remove_dir of a full one: {err:?}");
    let err = File::open(sub.join("nothing")).unwrap_err();
    check!(err.kind() == io::ErrorKind::NotFound && err.raw_os_error() == Some(2), "open: {err:?}");
    let err = OpenOptions::new().write(true).create_new(true).open(&file).unwrap_err();
    check!(err.kind() == io::ErrorKind::AlreadyExists, "create_new: {err:?}");

    // An advisory lock, held and refused to a second open file.
    let a = File::open(&file).unwrap();
    let b = File::open(&file).unwrap();
    a.lock().unwrap();
    check!(b.try_lock().is_err(), "a second lock was granted");
    a.unlock().unwrap();
    b.try_lock().unwrap();

    // A name beyond ASCII, as bytes and back.
    let odd = sub.join(std::ffi::OsStr::from_bytes("na\u{ef}ve \u{540d}\u{524d}".as_bytes()));
    fs::write(&odd, b"").unwrap();
    let found = fs::read_dir(&sub).unwrap().any(|e| e.unwrap().file_name().as_bytes() == odd.file_name().unwrap().as_bytes());
    check!(odd.exists() && found, "a name beyond ASCII");

    // The working directory.
    let before = env::current_dir().unwrap();
    env::set_current_dir(&sub).unwrap();
    check!(Path::new("file.txt").exists(), "relative to the new cwd");
    env::set_current_dir(before).unwrap();
    fs::remove_dir_all(dir.join("a")).unwrap();
    check!(!dir.join("a").exists(), "remove_dir_all");
}

fn check_net() {
    // TCP over the loopback.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut s, peer) = listener.accept().unwrap();
        check!(peer.ip().is_loopback(), "peer {peer}");
        let mut buf = [0u8; 4];
        s.read_exact(&mut buf).unwrap();
        s.write_all(&buf.map(|b| b.to_ascii_uppercase())).unwrap();
    });
    let mut c = TcpStream::connect(addr).unwrap();
    c.set_nodelay(true).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    c.write_all(b"ping").unwrap();
    let mut buf = [0u8; 4];
    c.read_exact(&mut buf).unwrap();
    check!(&buf == b"PING" && c.peer_addr().unwrap() == addr, "tcp echo");
    server.join().unwrap();

    // UDP.
    let a = UdpSocket::bind("127.0.0.1:0").unwrap();
    let b = UdpSocket::bind("127.0.0.1:0").unwrap();
    a.send_to(b"datagram", b.local_addr().unwrap()).unwrap();
    let mut buf = [0u8; 16];
    let (n, from) = b.recv_from(&mut buf).unwrap();
    check!(&buf[..n] == b"datagram" && from == a.local_addr().unwrap(), "udp");

    // A pair of unix sockets.
    let (mut x, mut y) = UnixStream::pair().unwrap();
    x.write_all(b"pair").unwrap();
    y.read_exact(&mut buf[..4]).unwrap();
    check!(&buf[..4] == b"pair" && x.as_raw_fd() != y.as_raw_fd(), "socketpair");
}

fn check_time_threads() {
    let t0 = Instant::now();
    thread::sleep(Duration::from_millis(30));
    let dt = t0.elapsed();
    check!(dt >= Duration::from_millis(30) && dt < Duration::from_secs(20), "slept {dt:?}");
    let year = 1970 + SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() / 31_557_600;
    check!((2024..2100).contains(&year), "the year is {year}");

    static DROPS: AtomicUsize = AtomicUsize::new(0);
    struct Local;
    impl Drop for Local {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }
    thread_local!(static L: Local = const { Local });
    let cpus = thread::available_parallelism().unwrap().get();
    check!((1..=1024).contains(&cpus), "available_parallelism {cpus}");
    let (tx, rx) = mpsc::channel();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let (tx, gate) = (tx.clone(), gate.clone());
            thread::Builder::new()
                .name(format!("worker{i}"))
                .spawn(move || {
                    L.with(|_| ());
                    let (lock, cv) = &*gate;
                    let mut go = lock.lock().unwrap();
                    while !*go {
                        go = cv.wait(go).unwrap();
                    }
                    check!(thread::current().name() == Some(&format!("worker{i}")), "thread name");
                    tx.send(i).unwrap();
                })
                .unwrap()
        })
        .collect();
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    for t in threads {
        t.join().unwrap();
    }
    drop(tx);
    let mut got: Vec<i32> = rx.iter().collect();
    got.sort();
    check!(got == (0..8).collect::<Vec<_>>(), "channel {got:?}");
    check!(DROPS.load(Ordering::SeqCst) == 8, "thread-local drops {}", DROPS.load(Ordering::SeqCst));
    // Two maps hash with different keys (the random source works).
    let h = |m: &HashMap<u32, u32>| {
        use std::hash::BuildHasher;
        m.hasher().hash_one(1u32)
    };
    check!(h(&HashMap::new()) != h(&HashMap::new()), "hash keys are not random");
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.get(1).map(String::as_str) == Some("child") {
        child(&args[2..]);
    }
    let dir = env::temp_dir().join(format!("std-unix-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    check_args_env();
    check_process(&dir);
    check_fs(&dir);
    check_net();
    check_time_threads();
    let _ = io::stdin().is_terminal();
    fs::remove_dir_all(&dir).unwrap();
    println!("[ OK ] std unix");
}
