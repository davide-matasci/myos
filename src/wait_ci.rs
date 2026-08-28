struct CiExpect {
    timeout: Duration,
    qemu_debug_exit: bool,
}

const CI_NEEDLES: [&str; 10] = [
    "Hello from myos",
    "heap ok",
    "int ok",
    "task a",
    "task b",
    "sched ok",
    "mod ok",
    "limine mod ok",
    "user ok",
    "fat ok",
];

fn serial_has_all_needles(serial: &str) -> bool {
    CI_NEEDLES.iter().all(|n| serial.contains(n))
}

fn wait_ci(mut child: Child, expect: CiExpect) {
    let mut stdout = child.stdout.take().expect("qemu stdout");
    let mut stderr = child.stderr.take().expect("qemu stderr");
    // Echo serial as it arrives so a timeout still leaves Limine output in CI logs.
    let stdout_handle = std::thread::spawn(move || {
        let mut s = String::new();
        let mut buf = [0u8; 256];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]);
                    eprint!("{chunk}");
                    s.push_str(&chunk);
                }
                Err(_) => break,
            }
        }
        s
    });
    let stderr_handle = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait().expect("failed to wait on qemu") {
            Some(status) => break status,
            None if started.elapsed() > expect.timeout => {
                let _ = child.kill();
                timed_out = true;
                break child.wait().expect("wait after kill");
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    let serial = stdout_handle.join().expect("stdout thread");
    let err = stderr_handle.join().expect("stderr thread");
    eprint!("{err}");

    if !serial_has_all_needles(&serial) {
        if timed_out {
            eprintln!("error: QEMU timed out after {:?}", expect.timeout);
            if serial.is_empty() {
                eprintln!("error: serial was empty at timeout");
            }
        }
        for needle in CI_NEEDLES {
            if !serial.contains(needle) {
                eprintln!("error: serial output did not contain {needle:?}");
            }
        }
        exit(1);
    }
    if expect.qemu_debug_exit && !timed_out {
        if status.code() != Some(QEMU_SUCCESS_STATUS) {
            eprintln!(
                "error: unexpected QEMU exit status {status:?} (want {QEMU_SUCCESS_STATUS} from isa-debug-exit)"
            );
            exit(1);
        }
    }
}
