//! End-to-end tests. Each test generates its own config on unique ports and
//! runs its backends in-process, so tests are parallel-safe, need no root,
//! and never touch processes they didn't spawn.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A minimal in-process HTTP/1.1 backend that answers every request with a
/// fixed body. Returns the port it listens on.
fn spawn_backend(body: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                // Read until end of request headers.
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

/// A backend that captures the raw bytes of every request it receives (so
/// tests can assert on what headers the proxy actually forwarded) and
/// replies with a fixed body plus an optional extra response header.
fn spawn_capturing_backend(
    body: &'static str,
    extra_response_header: Option<(&'static str, &'static str)>,
) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>) {
    use std::sync::{Arc, Mutex};

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().unwrap().port();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let store = captured.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let store = store.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let mut req = Vec::new();
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                store.lock().unwrap().push(req);
                let extra = extra_response_header
                    .map(|(k, v)| format!("{}: {}\r\n", k, v))
                    .unwrap_or_default();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                    body.len(),
                    extra,
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            });
        }
    });
    (port, captured)
}

/// A keep-alive backend that serves many requests per connection and counts
/// how many connections it has accepted.
fn spawn_keepalive_backend(
    body: &'static str,
) -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().unwrap().port();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = connections.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            counter.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let mut req = Vec::new();
                loop {
                    // Serve requests on this connection until the peer closes.
                    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => req.extend_from_slice(&buf[..n]),
                        }
                    }
                    req.clear();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    if stream.write_all(response.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    (port, connections)
}

/// A backend that waits `delay_ms` before responding (for drain tests).
fn spawn_slow_backend(body: &'static str, delay_ms: u64) -> u16 {
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
                std::thread::sleep(Duration::from_millis(delay_ms));
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

/// A backend that accepts connections but never responds (for timeout tests).
fn spawn_hanging_backend() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            held.push(stream);
        }
    });
    port
}

/// Hand out ports for proxy configs. A shared counter (offset by pid so two
/// test binaries can coexist) guarantees parallel tests in this process never
/// get the same port; the bind check skips ports taken by other processes.
fn free_port() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT_PORT: AtomicU16 = AtomicU16::new(0);
    // Stay below the OS ephemeral range (49152+ on macOS/Linux) so client
    // and backend sockets the OS hands out can never collide with us.
    let base = 20000 + (std::process::id() % 9000) as u16;
    loop {
        let port = base + NEXT_PORT.fetch_add(1, Ordering::Relaxed) % 20000;
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
}

struct TestProxy {
    process: Child,
    _config_dir: tempdir::TempDir,
}

// Tiny tempdir helper so we don't need the tempfile crate.
mod tempdir {
    pub struct TempDir(std::path::PathBuf);
    impl TempDir {
        pub fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "torana-test-{}-{}",
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

impl TestProxy {
    /// Write `config` to a temp file, start the proxy, and wait until
    /// `ready_port` accepts connections. Fails (with the proxy's stderr) if
    /// the process dies or never listens.
    fn try_start(config: &str, ready_port: u16) -> Result<Self, String> {
        let dir = tempdir::TempDir::new();
        let config_path = dir.path().join("torana.toml");
        let stderr_path = dir.path().join("stderr.log");
        std::fs::write(&config_path, config).unwrap();

        let process = Command::new(env!("CARGO_BIN_EXE_torana"))
            .args(["--config", config_path.to_str().unwrap()])
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&stderr_path).unwrap())
            .spawn()
            .expect("Failed to start proxy");

        let mut proxy = Self {
            process,
            _config_dir: dir,
        };

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", ready_port)).is_ok() {
                return Ok(proxy);
            }
            if let Ok(Some(status)) = proxy.process.try_wait() {
                return Err(format!(
                    "proxy exited with {} before listening on port {}. Stderr:\n{}",
                    status,
                    ready_port,
                    std::fs::read_to_string(&stderr_path).unwrap_or_default()
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(format!(
            "proxy did not listen on port {} within 10s. Stderr:\n{}",
            ready_port,
            std::fs::read_to_string(&stderr_path).unwrap_or_default()
        ))
    }
}

impl Drop for TestProxy {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// Allocate fresh ports and start the proxy, retrying with new ports when a
/// transient collision (e.g. another process grabbed the port between
/// allocation and bind) makes startup fail. Returns (proxy, listen, metrics).
fn start_proxy(make_config: impl Fn(u16, u16) -> String) -> (TestProxy, u16, u16) {
    let mut last_err = String::new();
    for _ in 0..3 {
        let listen_port = free_port();
        let metrics_port = free_port();
        match TestProxy::try_start(&make_config(listen_port, metrics_port), listen_port) {
            Ok(proxy) => return (proxy, listen_port, metrics_port),
            Err(e) => last_err = e,
        }
    }
    panic!("proxy failed to start after 3 attempts: {}", last_err);
}

fn http_config(listen_port: u16, metrics_port: u16, upstreams: &[(u16, u32)]) -> String {
    let upstream_entries: Vec<String> = upstreams
        .iter()
        .map(|(port, weight)| {
            format!(
                "  {{ addr = \"http://127.0.0.1:{}\", weight = {} }},",
                port, weight
            )
        })
        .collect();
    format!(
        r#"
[global]
log_level = "error"
metrics_addr = "127.0.0.1:{metrics_port}"

[[listener]]
addr = "127.0.0.1:{listen_port}"
protocol = "http"

[[route]]
name = "default"
upstream = [
{upstreams}
]
"#,
        metrics_port = metrics_port,
        listen_port = listen_port,
        upstreams = upstream_entries.join("\n")
    )
}

#[test]
fn http_proxy_forwards_to_backend() {
    let backend_port = spawn_backend("Hello from backend");
    let (_proxy, listen_port, _) = start_proxy(|l, m| http_config(l, m, &[(backend_port, 100)]));

    let response = reqwest::blocking::Client::new()
        .get(format!("http://127.0.0.1:{}/test.txt", listen_port))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("request failed");

    assert_eq!(response.status(), 200);
    assert_eq!(response.text().unwrap(), "Hello from backend");
}

#[test]
fn load_balancing_hits_all_upstreams() {
    let backend_a = spawn_backend("Backend A");
    let backend_b = spawn_backend("Backend B");
    let (_proxy, listen_port, _) =
        start_proxy(|l, m| http_config(l, m, &[(backend_a, 50), (backend_b, 50)]));

    let client = reqwest::blocking::Client::new();
    let mut bodies = std::collections::HashSet::new();
    for i in 0..10 {
        let response = client
            .get(format!("http://127.0.0.1:{}/", listen_port))
            .timeout(Duration::from_secs(5))
            .send()
            .unwrap_or_else(|e| panic!("request {} failed: {}", i, e));
        assert_eq!(response.status(), 200);
        bodies.insert(response.text().unwrap());
    }
    assert!(
        bodies.contains("Backend A") && bodies.contains("Backend B"),
        "expected both backends to serve traffic, got: {:?}",
        bodies
    );
}

#[test]
fn https_listener_terminates_tls() {
    let backend_port = spawn_backend("HTTPS Backend");

    let dir = tempdir::TempDir::new();
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_path = dir.path().join("tls.crt");
    let key_path = dir.path().join("tls.key");
    std::fs::write(&cert_path, cert.cert.pem()).unwrap();
    std::fs::write(&key_path, cert.key_pair.serialize_pem()).unwrap();

    let (_proxy, listen_port, _) = start_proxy(|listen_port, metrics_port| {
        format!(
            r#"
[global]
log_level = "error"
metrics_addr = "127.0.0.1:{metrics_port}"

[[listener]]
addr = "127.0.0.1:{listen_port}"
protocol = "https"
tls_cert = "{cert_path}"
tls_key = "{key_path}"

[[route]]
name = "default"
upstream = [{{ addr = "http://127.0.0.1:{backend_port}" }}]
"#,
            metrics_port = metrics_port,
            listen_port = listen_port,
            cert_path = cert_path.display(),
            key_path = key_path.display(),
            backend_port = backend_port,
        )
    });

    let client = reqwest::blocking::Client::builder()
        .use_rustls_tls()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let response = client
        .get(format!("https://127.0.0.1:{}/", listen_port))
        .send()
        .expect("https request failed");

    assert_eq!(response.status(), 200);
    assert_eq!(response.text().unwrap(), "HTTPS Backend");
}

#[test]
fn mtls_requires_and_forwards_verified_client_cert() {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};

    let (backend_port, captured) = spawn_capturing_backend("mtls ok", None);
    let dir = tempdir::TempDir::new();

    // A tiny CA, and a server cert + client cert both signed by it.
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(vec![]).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();

    let server_key = KeyPair::generate().unwrap();
    let server_params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .unwrap();

    let client_key = KeyPair::generate().unwrap();
    let client_params = CertificateParams::new(vec!["torana-test-client".to_string()]).unwrap();
    let client_cert = client_params
        .signed_by(&client_key, &ca_cert, &ca_key)
        .unwrap();

    let ca_path = dir.path().join("ca.crt");
    let server_cert_path = dir.path().join("server.crt");
    let server_key_path = dir.path().join("server.key");
    let client_cert_path = dir.path().join("client.crt");
    let client_key_path = dir.path().join("client.key");
    std::fs::write(&ca_path, ca_cert.pem()).unwrap();
    std::fs::write(&server_cert_path, server_cert.pem()).unwrap();
    std::fs::write(&server_key_path, server_key.serialize_pem()).unwrap();
    std::fs::write(&client_cert_path, client_cert.pem()).unwrap();
    std::fs::write(&client_key_path, client_key.serialize_pem()).unwrap();

    let (_proxy, listen_port, _) = start_proxy(|listen_port, metrics_port| {
        format!(
            r#"
[global]
log_level = "error"
metrics_addr = "127.0.0.1:{metrics_port}"

[[listener]]
addr = "127.0.0.1:{listen_port}"
protocol = "https"
tls_cert = "{server_cert_path}"
tls_key = "{server_key_path}"
tls_client_ca = "{ca_path}"

[[route]]
name = "default"
upstream = [{{ addr = "http://127.0.0.1:{backend_port}" }}]
"#,
            metrics_port = metrics_port,
            listen_port = listen_port,
            server_cert_path = server_cert_path.display(),
            server_key_path = server_key_path.display(),
            ca_path = ca_path.display(),
            backend_port = backend_port,
        )
    });

    // A client with no certificate at all must be rejected at the TLS
    // handshake, not merely at the HTTP layer.
    let no_cert_client = reqwest::blocking::Client::builder()
        .use_rustls_tls()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    assert!(
        no_cert_client
            .get(format!("https://127.0.0.1:{}/", listen_port))
            .send()
            .is_err(),
        "a connection without a client certificate must be rejected"
    );

    // A client presenting a cert signed by the configured CA is accepted,
    // and the proxy forwards a fingerprint of that verified certificate.
    let client_identity_pem = format!("{}\n{}", client_cert.pem(), client_key.serialize_pem());
    let identity = reqwest::Identity::from_pem(client_identity_pem.as_bytes()).unwrap();
    let mtls_client = reqwest::blocking::Client::builder()
        .use_rustls_tls()
        .danger_accept_invalid_certs(true)
        .identity(identity)
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let response = mtls_client
        .get(format!("https://127.0.0.1:{}/", listen_port))
        .send()
        .expect("mTLS request with a valid client cert should succeed");
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().unwrap(), "mtls ok");

    let requests = captured.lock().unwrap();
    let raw = String::from_utf8_lossy(&requests[0]);
    assert!(
        raw.to_lowercase().contains("x-client-cert-fingerprint:"),
        "raw request should carry the verified client cert fingerprint:\n{}",
        raw
    );
}

#[test]
fn dead_upstream_returns_502() {
    let dead_port = free_port();
    let (_proxy, listen_port, _) = start_proxy(|l, m| http_config(l, m, &[(dead_port, 100)]));

    let response = reqwest::blocking::Client::new()
        .get(format!("http://127.0.0.1:{}/", listen_port))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("request failed");

    assert_eq!(response.status(), 502);
}

#[test]
fn hanging_upstream_returns_504() {
    let backend_port = spawn_hanging_backend();
    let (_proxy, listen_port, _) = start_proxy(|l, m| {
        let mut config = http_config(l, m, &[(backend_port, 100)]);
        config.push_str("\n[route.timeout]\ntotal = \"500ms\"\n");
        config
    });

    let start = Instant::now();
    let response = reqwest::blocking::Client::new()
        .get(format!("http://127.0.0.1:{}/", listen_port))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("request failed");

    assert_eq!(response.status(), 504);
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "timeout should trigger at ~500ms"
    );
}

#[test]
fn upstream_connections_are_reused() {
    use std::sync::atomic::Ordering;

    let (backend_port, connections) = spawn_keepalive_backend("pooled");
    let (_proxy, listen_port, _) = start_proxy(|l, m| http_config(l, m, &[(backend_port, 100)]));

    let client = reqwest::blocking::Client::new();
    for i in 0..20 {
        let response = client
            .get(format!("http://127.0.0.1:{}/", listen_port))
            .timeout(Duration::from_secs(5))
            .send()
            .unwrap_or_else(|e| panic!("request {} failed: {}", i, e));
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().unwrap(), "pooled");
    }

    let opened = connections.load(Ordering::SeqCst);
    assert!(
        opened < 20,
        "expected pooled upstream connections, but 20 sequential requests \
         opened {} connections",
        opened
    );
}

#[test]
fn metrics_endpoint_reports_requests() {
    let backend_port = spawn_backend("ok");
    let (_proxy, listen_port, metrics_port) =
        start_proxy(|l, m| http_config(l, m, &[(backend_port, 100)]));

    let client = reqwest::blocking::Client::new();
    client
        .get(format!("http://127.0.0.1:{}/", listen_port))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("request failed");

    let metrics = client
        .get(format!("http://127.0.0.1:{}/metrics", metrics_port))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("metrics request failed")
        .text()
        .unwrap();

    assert!(
        metrics.contains("http_requests_total"),
        "metrics output missing counters: {}",
        metrics
    );
    assert!(metrics.contains("http_request_duration_seconds"));
}

#[test]
fn graceful_shutdown_drains_inflight_requests() {
    let backend_port = spawn_slow_backend("drained ok", 700);
    let (mut proxy, listen_port, _) = start_proxy(|l, m| http_config(l, m, &[(backend_port, 100)]));

    // Fire a request that will still be in flight when SIGTERM arrives.
    let request = std::thread::spawn(move || {
        reqwest::blocking::Client::new()
            .get(format!("http://127.0.0.1:{}/", listen_port))
            .timeout(Duration::from_secs(5))
            .send()
    });
    std::thread::sleep(Duration::from_millis(200));

    let _ = Command::new("kill")
        .args(["-TERM", &proxy.process.id().to_string()])
        .status();

    // The in-flight response must complete despite the shutdown...
    let response = request
        .join()
        .unwrap()
        .expect("in-flight request should complete during drain");
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().unwrap(), "drained ok");

    // ...and the process must then exit cleanly on its own.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(Some(status)) = proxy.process.try_wait() {
            assert!(status.success(), "proxy exited with {}", status);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "proxy did not exit within 5s of SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn healthz_endpoint_responds() {
    let backend_port = spawn_backend("ok");
    let (_proxy, _listen, metrics_port) =
        start_proxy(|l, m| http_config(l, m, &[(backend_port, 100)]));

    let response = reqwest::blocking::Client::new()
        .get(format!("http://127.0.0.1:{}/healthz", metrics_port))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("healthz request failed");
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().unwrap(), "ok");
}

#[test]
fn header_rewrite_applies_request_and_response_overrides() {
    let (backend_port, captured) =
        spawn_capturing_backend("ok", Some(("x-backend-secret", "leak")));
    let (_proxy, listen_port, _) = start_proxy(|l, m| {
        format!(
            r#"
[global]
log_level = "error"
metrics_addr = "127.0.0.1:{m}"

[[listener]]
addr = "127.0.0.1:{l}"
protocol = "http"

[[route]]
name = "default"
upstream = [{{ addr = "http://127.0.0.1:{backend_port}" }}]

[route.headers]
request = {{ "x-injected" = "proxy-value", "x-remove-me" = "" }}
response = {{ "x-backend-secret" = "", "x-added" = "yes" }}
"#,
        )
    });

    let response = reqwest::blocking::Client::new()
        .get(format!("http://127.0.0.1:{}/", listen_port))
        .header("x-remove-me", "should-be-stripped")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("request failed");

    assert_eq!(response.status(), 200);
    assert_eq!(response.headers().get("x-added").unwrap(), "yes");
    assert!(
        response.headers().get("x-backend-secret").is_none(),
        "response override should have removed x-backend-secret"
    );

    let requests = captured.lock().unwrap();
    let raw = String::from_utf8_lossy(&requests[0]);
    assert!(
        raw.contains("x-injected: proxy-value"),
        "raw request:\n{}",
        raw
    );
    assert!(!raw.contains("x-remove-me"), "raw request:\n{}", raw);
}

#[test]
fn mirror_duplicates_replayable_requests_without_affecting_response() {
    let (main_port, main_captured) = spawn_capturing_backend("main-response", None);
    let (mirror_port, mirror_captured) = spawn_capturing_backend("mirror-response", None);
    let (_proxy, listen_port, _) = start_proxy(|l, m| {
        format!(
            r#"
[global]
log_level = "error"
metrics_addr = "127.0.0.1:{m}"

[[listener]]
addr = "127.0.0.1:{l}"
protocol = "http"

[[route]]
name = "default"
upstream = [{{ addr = "http://127.0.0.1:{main_port}" }}]

[route.mirror]
addr = "http://127.0.0.1:{mirror_port}"
"#,
        )
    });

    let response = reqwest::blocking::Client::new()
        .get(format!("http://127.0.0.1:{}/", listen_port))
        .timeout(Duration::from_secs(5))
        .send()
        .expect("request failed");
    assert_eq!(response.text().unwrap(), "main-response");

    // Mirroring is fire-and-forget; give the background task a moment.
    let deadline = Instant::now() + Duration::from_secs(3);
    while mirror_captured.lock().unwrap().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }

    assert_eq!(main_captured.lock().unwrap().len(), 1);
    assert_eq!(
        mirror_captured.lock().unwrap().len(),
        1,
        "mirror backend should have received a duplicated request"
    );
}

#[test]
fn mirror_is_skipped_for_requests_with_a_body() {
    let (main_port, _main_captured) = spawn_capturing_backend("ok", None);
    let (mirror_port, mirror_captured) = spawn_capturing_backend("ok", None);
    let (_proxy, listen_port, _) = start_proxy(|l, m| {
        format!(
            r#"
[global]
log_level = "error"
metrics_addr = "127.0.0.1:{m}"

[[listener]]
addr = "127.0.0.1:{l}"
protocol = "http"

[[route]]
name = "default"
upstream = [{{ addr = "http://127.0.0.1:{main_port}" }}]

[route.mirror]
addr = "http://127.0.0.1:{mirror_port}"
"#,
        )
    });

    let response = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{}/", listen_port))
        .body("a body makes this request non-replayable")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("request failed");
    assert_eq!(response.status(), 200);

    // Give a wrongly-spawned mirror task time to show up before asserting
    // its absence.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        mirror_captured.lock().unwrap().len(),
        0,
        "a request with a body must never be mirrored"
    );
}

#[test]
fn invalid_config_exits_nonzero() {
    let dir = tempdir::TempDir::new();
    let config_path = dir.path().join("bad.toml");
    // https:// upstreams are rejected until upstream TLS exists.
    std::fs::write(
        &config_path,
        r#"
[global]
log_level = "error"

[[listener]]
addr = "127.0.0.1:1"
protocol = "http"

[[route]]
name = "default"
upstream = [{ addr = "https://example.com" }]
"#,
    )
    .unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_torana"))
        .args(["--config", config_path.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("failed to run proxy");

    assert!(!status.success(), "proxy should reject invalid config");
}
