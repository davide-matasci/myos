//! The test web server: what the guest's network tests fetch instead of a
//! site on the internet, whose answers vary (a rate limit, a 503) from run
//! to run. Plain HTTP on 127.0.0.1:[`HTTP_PORT`] and HTTPS on
//! 127.0.0.1:[`HTTPS_PORT`], which the guest reaches through QEMU's user
//! network as 10.0.2.2. HTTPS presents `user/tests/tls/server.pem`, signed
//! by the test CA the guest's tests trust alone
//! (`/lib/myos-tests/test-ca.pem`).
//!
//! Both serve `/hello.txt`, [`HELLO`].

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

pub const HTTP_PORT: u16 = 8766;
pub const HTTPS_PORT: u16 = 8767;
pub const HELLO: &str = "hello from the myos test server\n";

/// Serve both ports in background threads. A port that is taken is
/// reported and left out: the tests that need it fail then.
pub fn start(root: &Path) {
    let tls = match tls_config(&root.join("user/tests/tls")) {
        Ok(c) => Some(Arc::new(c)),
        Err(e) => {
            eprintln!("test www: no HTTPS ({e})");
            None
        }
    };
    serve(HTTP_PORT, None);
    if let Some(tls) = tls {
        serve(HTTPS_PORT, Some(tls));
    }
}

fn serve(port: u16, tls: Option<Arc<rustls::ServerConfig>>) {
    let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) else {
        eprintln!("test www: port {port} busy, not serving");
        return;
    };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tls = tls.clone();
            std::thread::spawn(move || {
                let mut stream = stream;
                match tls {
                    Some(tls) => {
                        let Ok(mut conn) = rustls::ServerConnection::new(tls) else { return };
                        serve_one(&mut rustls::Stream::new(&mut conn, &mut stream));
                        conn.send_close_notify();
                        let _ = conn.complete_io(&mut stream);
                    }
                    None => serve_one(&mut stream),
                }
            });
        }
    });
}

fn tls_config(dir: &Path) -> Result<rustls::ServerConfig, String> {
    let certs = CertificateDer::pem_file_iter(dir.join("server.pem"))
        .and_then(|i| i.collect::<Result<Vec<_>, _>>())
        .map_err(|e| format!("server.pem: {e}"))?;
    let key = PrivateKeyDer::from_pem_file(dir.join("server.key")).map_err(|e| format!("server.key: {e}"))?;
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .and_then(|b| b.with_no_client_auth().with_single_cert(certs, key))
        .map_err(|e| e.to_string())
}

/// One request (a GET: its line, then the headers up to the blank line),
/// then the answer, after which the connection closes.
fn serve_one<S: Read + Write>(stream: &mut S) {
    let mut reader = BufReader::new(&mut *stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
            break;
        }
    }
    drop(reader);
    let mut words = line.split_whitespace();
    let response = match (words.next(), words.next()) {
        (Some("GET"), Some("/hello.txt")) => reply("200 OK", HELLO.as_bytes()),
        _ => reply("404 Not Found", b""),
    };
    let _ = stream.write_all(&response);
    let _ = stream.flush();
}

fn reply(status: &str, body: &[u8]) -> Vec<u8> {
    let mut r = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    r.extend_from_slice(body);
    r
}
