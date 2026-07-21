//! Real end-to-end ACME test against a local Pebble server (Let's
//! Encrypt's own ACME protocol test implementation — RFC 8555, not a mock).
//!
//! Not part of the default `cargo test` run: requires Docker and the
//! `acme` feature. Run via `scripts/test-acme-e2e.sh`, which starts Pebble,
//! runs this test, and tears Pebble down again.
#![cfg(feature = "acme")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PEBBLE_DIRECTORY_URL: &str = "https://127.0.0.1:14000/dir";
const PEBBLE_ROOTS_URL: &str = "https://127.0.0.1:15000/roots/0";
// Fixed test CA baked into every Pebble image, used only for Pebble's own
// API TLS — not the certificate-issuing root, which Pebble regenerates
// every run and is fetched from PEBBLE_ROOTS_URL below.
const PEBBLE_MINICA_PEM: &str = include_str!("fixtures/pebble-minica.pem");

mod tempdir {
    pub struct TempDir(std::path::PathBuf);
    impl TempDir {
        pub fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "torana-acme-e2e-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn spawn_backend(body: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let mut req = Vec::new();
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            });
        }
    });
    port
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Fetches Pebble's certificate-issuing root, which is regenerated fresh on
/// every container start (unlike the fixed minica API cert above). Trusts
/// nothing when fetching, since this is a bootstrap step against localhost
/// purely to obtain the root we then use for the test's real verification.
fn fetch_pebble_issuing_root() -> String {
    let client = reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    client
        .get(PEBBLE_ROOTS_URL)
        .send()
        .expect("could not reach Pebble management API — is it running? (scripts/test-acme-e2e.sh)")
        .text()
        .expect("reading Pebble issuing root")
}

fn wait_for_pebble() {
    let client = reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if client.get(PEBBLE_DIRECTORY_URL).send().is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Pebble did not become reachable at {} within 30s",
            PEBBLE_DIRECTORY_URL
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

struct TestProxy {
    process: Child,
}
impl Drop for TestProxy {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[test]
#[ignore = "requires Docker + a running Pebble instance; run via scripts/test-acme-e2e.sh"]
fn acme_issues_and_serves_a_real_certificate_from_pebble() {
    wait_for_pebble();

    let dir = tempdir::TempDir::new();
    let minica_path = dir.path().join("pebble-minica.pem");
    std::fs::write(&minica_path, PEBBLE_MINICA_PEM).unwrap();
    let cache_dir = dir.path().join("acme-cache");

    let backend_port = spawn_backend("acme e2e ok");
    let listen_port = free_port();
    let metrics_port = free_port();
    let domain = format!("torana-e2e-{}.example", std::process::id());

    let config_path = dir.path().join("torana.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[global]
log_level = "info"
metrics_addr = "127.0.0.1:{metrics_port}"

[[listener]]
addr = "127.0.0.1:{listen_port}"
protocol = "https"

[listener.acme]
domains = ["{domain}"]
directory_url = "{directory_url}"
cache_dir = "{cache_dir}"
ca_cert = "{minica_path}"

[[route]]
name = "default"
upstream = [{{ addr = "http://127.0.0.1:{backend_port}" }}]
"#,
            metrics_port = metrics_port,
            listen_port = listen_port,
            domain = domain,
            directory_url = PEBBLE_DIRECTORY_URL,
            cache_dir = cache_dir.display(),
            minica_path = minica_path.display(),
            backend_port = backend_port,
        ),
    )
    .unwrap();

    let stderr_path = dir.path().join("stderr.log");
    let process = Command::new(env!("CARGO_BIN_EXE_torana"))
        .args(["--config", config_path.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&stderr_path).unwrap())
        .spawn()
        .expect("failed to start torana");
    let proxy = TestProxy { process };

    // Wait for the listener to accept TCP connections at all (near
    // instant), independent of how long ACME issuance takes.
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(("127.0.0.1", listen_port)).is_err() {
        assert!(
            Instant::now() < deadline,
            "torana never opened its HTTPS listener"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // The first handshake(s) block on real ACME issuance (order, TLS-ALPN-01
    // challenge, possibly a retry after Pebble's deliberately-injected
    // ~5% bad-nonce rate), so poll instead of expecting an instant 200.
    let pebble_issuing_root =
        reqwest::Certificate::from_pem(fetch_pebble_issuing_root().as_bytes())
            .expect("parsing Pebble's issuing root");
    let client = reqwest::blocking::Client::builder()
        .add_root_certificate(pebble_issuing_root)
        .resolve(&domain, ([127, 0, 0, 1], listen_port).into())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(60);
    let response = loop {
        match client
            .get(format!("https://{}:{}/", domain, listen_port))
            .send()
        {
            Ok(response) => break response,
            Err(e) => {
                assert!(
                    Instant::now() < deadline,
                    "ACME issuance did not complete within 60s; last error: {}. Proxy stderr:\n{}",
                    e,
                    std::fs::read_to_string(&stderr_path).unwrap_or_default()
                );
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    };

    // Positive proof: the served certificate validates when Pebble's
    // issuing root is trusted, and the request actually reached our
    // backend through the proxy — this is a genuine RFC 8555 issuance,
    // not a self-signed fallback or a mocked response.
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().unwrap(), "acme e2e ok");

    // Negative control: the exact same request, without trusting Pebble's
    // root, must fail TLS verification. This proves the positive result
    // above is actually caused by trusting Pebble's root — not by some
    // accidental default trust that would make the test pass regardless.
    let untrusting_client = reqwest::blocking::Client::builder()
        .resolve(&domain, ([127, 0, 0, 1], listen_port).into())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let control = untrusting_client
        .get(format!("https://{}:{}/", domain, listen_port))
        .send();
    let control_err = control.expect_err(
        "request without trusting Pebble's root should fail TLS verification, but succeeded",
    );
    assert!(
        control_err.is_connect(),
        "expected a TLS/connect failure specifically (not e.g. a DNS error), got: {}",
        control_err
    );

    drop(proxy);
}
