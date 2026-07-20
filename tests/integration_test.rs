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
