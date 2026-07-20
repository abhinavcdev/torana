use std::process::{Command, Child, Stdio};
use std::time::Duration;
use std::thread;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};

// Global flag to ensure tests run sequentially to avoid port conflicts
static TEST_RUNNING: AtomicBool = AtomicBool::new(false);

struct TestGuard;
impl TestGuard {
    fn acquire() -> Self {
        while TEST_RUNNING.compare_exchange_weak(false, true, Ordering::SeqCst, Ordering::Relaxed).is_err() {
            thread::sleep(Duration::from_millis(100));
        }
        TestGuard
    }
}
impl Drop for TestGuard {
    fn drop(&mut self) {
        TEST_RUNNING.store(false, Ordering::SeqCst);
    }
}

struct TestProxy {
    process: Child,
}

impl TestProxy {
    fn start(config_path: &str) -> Self {
        // Use the debug binary that was built
        let binary_path = env!("CARGO_BIN_EXE_caddyrs");
        let process = Command::new(binary_path)
            .args(&["--config", config_path])
            .env("RUST_LOG", "error")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("Failed to start proxy");

        thread::sleep(Duration::from_millis(3000));
        Self { process }
    }
}

impl Drop for TestProxy {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn is_port_available(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok() && TcpListener::bind(("::1", port)).is_ok()
}

fn cleanup_processes() {
    // Kill any remaining processes from previous tests
    let _ = Command::new("pkill").arg("-9").arg("-f").arg("caddyrs").output();
    thread::sleep(Duration::from_millis(300));
    let _ = Command::new("pkill").arg("-9").arg("-f").arg("http.server").output();
    thread::sleep(Duration::from_millis(300));
    let _ = Command::new("pkill").arg("-9").arg("python3").output();
    thread::sleep(Duration::from_millis(800));

    // Wait for ports to be released - with lsof check for Mac compatibility
    let mut attempts = 0;
    loop {
        let all_available = is_port_available(80) && is_port_available(443) &&
           is_port_available(18080) && is_port_available(18081) &&
           is_port_available(9999);

        if all_available {
            thread::sleep(Duration::from_millis(300));
            return;
        }

        attempts += 1;
        if attempts > 80 {
            eprintln!("Warning: ports still in use after cleanup attempts");
            // List what's using the ports
            let _ = Command::new("sh")
                .arg("-c")
                .arg("lsof -i :80 -i :443 -i :18080 -i :18081 -i :9999 2>/dev/null | tail -5 || true")
                .status();
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_port(port: u16, timeout_secs: u64) {
    let start = std::time::Instant::now();
    loop {
        if !is_port_available(port) {
            return;
        }
        if start.elapsed().as_secs() > timeout_secs {
            panic!("Port {} did not become available within {} seconds", port, timeout_secs);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn test_http_proxy_basic() {
    let _guard = TestGuard::acquire();
    cleanup_processes();

    // Create a simple test file
    let test_content = "Hello from backend";
    let _ = std::fs::write("/tmp/test.txt", test_content);

    // Start simple backend on port 9999
    let _backend = Command::new("python3")
        .args(&["-m", "http.server", "9999", "--directory", "/tmp"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start backend");

    thread::sleep(Duration::from_millis(2000));

    let _proxy = TestProxy::start("tests/fixtures/basic.toml");
    wait_for_port(80, 5);

    thread::sleep(Duration::from_millis(1500));

    // Test request - retry logic for stability
    let mut last_error = None;
    for attempt in 0..15 {
        match reqwest::blocking::Client::new()
            .get("http://localhost:80/test.txt")
            .timeout(Duration::from_secs(5))
            .send()
        {
            Ok(response) => {
                let status = response.status();
                if status != 200 {
                    eprintln!("Attempt {}: Got status {}", attempt, status);
                    if attempt < 14 {
                        thread::sleep(Duration::from_millis(300));
                    }
                    last_error = Some(format!("Status {}", status));
                } else {
                    return;
                }
            }
            Err(e) => {
                last_error = Some(e.to_string());
                if attempt < 14 {
                    thread::sleep(Duration::from_millis(300));
                }
            }
        }
    }

    panic!("Failed after retries: {:?}", last_error);
}

#[test]
fn test_https_proxy() {
    let _guard = TestGuard::acquire();
    cleanup_processes();

    // Create a simple test file
    let _ = std::fs::write("/tmp/https_test.txt", "HTTPS Backend");

    // Start simple backend on port 9999
    let _backend = Command::new("python3")
        .args(&["-m", "http.server", "9999", "--directory", "/tmp"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok();

    thread::sleep(Duration::from_millis(2000));

    let _proxy = TestProxy::start("tests/fixtures/https.toml");
    wait_for_port(443, 5);

    thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    // Retry logic for HTTPS
    for attempt in 0..15 {
        match client
            .get("https://localhost:443/https_test.txt")
            .send()
        {
            Ok(response) => {
                let status = response.status();
                if status != 200 {
                    eprintln!("Attempt {}: Got status {}", attempt, status);
                    if attempt < 14 {
                        thread::sleep(Duration::from_millis(300));
                    }
                } else {
                    return;
                }
            }
            Err(e) => {
                eprintln!("Attempt {}: Error: {}", attempt, e);
                if attempt < 14 {
                    thread::sleep(Duration::from_millis(300));
                }
            }
        }
    }

    panic!("HTTPS request failed after retries");
}

#[test]
fn test_load_balancing() {
    let _guard = TestGuard::acquire();
    cleanup_processes();

    // Create test files
    let _ = std::fs::write("/tmp/backend1.txt", "Backend 1");
    let _ = std::fs::write("/tmp/backend2.txt", "Backend 2");

    // Start 2 backends on different ports
    let _b1 = Command::new("python3")
        .args(&["-m", "http.server", "18080", "--directory", "/tmp"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok();

    let _b2 = Command::new("python3")
        .args(&["-m", "http.server", "18081", "--directory", "/tmp"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok();

    thread::sleep(Duration::from_millis(2500));

    let _proxy = TestProxy::start("tests/fixtures/lb.toml");
    wait_for_port(80, 5);

    thread::sleep(Duration::from_millis(2000));

    // Make 10 requests - all should succeed
    for i in 0..10 {
        match reqwest::blocking::Client::new()
            .get("http://localhost:80/backend1.txt")
            .timeout(Duration::from_secs(5))
            .send()
        {
            Ok(response) => {
                assert_eq!(response.status(), 200, "Request {} failed with status {}", i, response.status());
            }
            Err(e) => {
                panic!("Request {} failed: {}", i, e);
            }
        }
    }
}
