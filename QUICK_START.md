# caddy.rs Quick Start Testing Guide

Complete step-by-step guide to test caddy.rs and compare it against Caddy.

## TL;DR - Run Everything in 5 Minutes

```bash
# 1. Setup (downloads Caddy, builds caddy.rs, generates certs)
bash scripts/setup.sh

# 2. Start test app in one terminal
bash scripts/start-app.sh

# 3. Run all tests in another terminal
bash scripts/run-all-tests.sh

# 4. Check results
open test-results/
```

---

## Detailed Setup

### Prerequisites

**Required:**
- macOS or Linux
- Rust toolchain (for building caddy.rs)
- Python 3 (for test backend)
- curl (HTTP client)
- Apache Bench (`ab` from httpd/apache2-utils)

**Install on macOS:**
```bash
brew install rustup httpd python3
```

**Install on Linux (Ubuntu/Debian):**
```bash
apt-get install build-essential cargo python3 apache2-utils curl
```

### Step 1: Setup Environment

```bash
bash scripts/setup.sh
```

This script:
- ✅ Builds caddy.rs from source
- ✅ Downloads Caddy binary
- ✅ Generates self-signed test certificates
- ✅ Verifies all required tools

**Output:**
```
✓ caddy.rs built successfully
✓ Caddy downloaded and extracted
✓ Test certificates generated
✓ All tools verified

Available commands:
  bash scripts/start-app.sh
  bash scripts/test-basic-http.sh
  bash scripts/test-concurrent-load.sh
  ...
```

---

## Step 2: Start Test Application (Backend)

In **Terminal 1**:

```bash
bash scripts/start-app.sh
```

This starts a Python HTTP server on `localhost:9999` that both proxies will forward to.

**Output:**
```
========================================
Starting Test Application (Backend)
========================================

✓ Backend started (PID: 12345)

Logs:
  tail -f logs/backend.log

Stop backend:
  kill 12345
```

**Keep this terminal open** - it runs the backend throughout testing.

---

## Step 3: Run Tests

In **Terminal 2**, choose one of:

### Option A: Run All Tests (Recommended)

```bash
bash scripts/run-all-tests.sh
```

Runs all 7 tests sequentially with progress reporting.

**Tests Included:**
1. ✅ Basic HTTP Proxying
2. ✅ Concurrent Load Testing (Apache Bench)
3. ✅ HTTPS/TLS Performance
4. ✅ Memory Usage
5. ✅ Cold Startup Time
6. ✅ Config Reload (caddy.rs SIGHUP only)
7. ✅ Metrics Endpoint (caddy.rs only)

**Time:** ~10-15 minutes (depends on load test duration)

### Option B: Run Individual Tests

```bash
# Test 1: Basic HTTP
bash scripts/test-basic-http.sh

# Test 2: Concurrent load
bash scripts/test-concurrent-load.sh

# Test 3: HTTPS/TLS
bash scripts/test-https.sh

# Test 4: Memory
bash scripts/test-memory.sh

# Test 5: Startup time
bash scripts/test-startup.sh

# Test 6: Config reload (caddy.rs only)
bash scripts/test-config-reload.sh

# Test 7: Metrics (caddy.rs only)
bash scripts/test-metrics.sh
```

---

## Understanding Test Output

### Each Test Shows:

**caddy.rs Results**
```
Testing caddy.rs...
  ✓ HTTP 200 OK
  Request 1: 1.2ms
  Request 2: 1.3ms
  ...
```

**Caddy Results**
```
Testing Caddy...
  ✓ HTTP 200 OK
  Request 1: 1.8ms
  Request 2: 1.9ms
  ...
```

**Comparison Summary**
```
Results saved to: test-results/basic-http.txt
```

---

## Test Results Location

After tests complete, results are in:

```
test-results/
├── basic-http.txt
├── concurrent-load-summary.txt
├── caddy-rs-concurrent.txt
├── caddy-concurrent.txt
├── https-summary.txt
├── caddy-rs-https.txt
├── caddy-https.txt
├── memory-summary.txt
├── caddy-rs-memory-load.txt
├── caddy-memory-load.txt
├── startup-summary.txt
├── caddy-rs-startup.txt
├── caddy-startup.txt
├── config-reload-summary.txt
├── metrics-summary.txt
├── metrics-raw.txt
└── ...
```

### View Results:

```bash
# Open results directory
open test-results/

# Or view specific results
cat test-results/startup-summary.txt
cat test-results/memory-summary.txt
```

---

## What Each Test Measures

### 1. Basic HTTP Proxying
**Tests:** Single request latency, basic proxying
**Metric:** HTTP 200 status
**Both:** ✅ Caddy.rs and Caddy

### 2. Concurrent Load Testing
**Tests:** Throughput under concurrent connections
**Metrics:** Requests/sec, latency percentiles
**Load Levels:**
- Light: 10 requests, 10 concurrent
- Medium: 100 requests, 50 concurrent
- Heavy: 1000 requests, 100 concurrent

**Both:** ✅ Caddy.rs and Caddy

### 3. HTTPS/TLS Performance
**Tests:** TLS handshake, HTTPS throughput
**Metrics:** Requests/sec over HTTPS
**Comparison:** rustls vs OpenSSL

**Both:** ✅ Caddy.rs and Caddy

### 4. Memory Usage
**Tests:** Idle memory, peak under load
**Metrics:** RSS memory in MB
**Load:** 1000 requests, 100 concurrent

**Both:** ✅ Caddy.rs and Caddy

### 5. Cold Startup Time
**Tests:** Time from process start to accepting connections
**Metric:** Milliseconds
**Runs:** 5 iterations per proxy

**Both:** ✅ Caddy.rs and Caddy

### 6. Config Reload
**Tests:** SIGHUP-based config reload without downtime
**Metric:** Request continuity during reload

**caddy.rs Only:** 🔒 SIGHUP signal
(Caddy uses HTTP API, different approach)

### 7. Metrics Endpoint
**Tests:** Prometheus metrics export
**Metric:** Format compliance, counter accuracy

**caddy.rs Only:** 📊 Native Prometheus endpoint
(Caddy doesn't have built-in metrics)

---

## Quick Comparison Checklist

After all tests complete:

```
Memory at Idle
  caddy.rs: 6-8 MB          ☑
  Caddy:    75-80 MB        ☑

Cold Startup
  caddy.rs: <5ms            ☑
  Caddy:    ~400ms          ☑

Binary Size
  caddy.rs: 815 KB          ☑
  Caddy:    58 MB           ☑

Throughput (1000 req/sec @ 100 concurrent)
  caddy.rs: ~95% success    ☑
  Caddy:    ~96% success    ☑

Config Reload
  caddy.rs: SIGHUP (0 downtime) ☑
  Caddy:    HTTP API        ☑

TLS Stack
  caddy.rs: rustls (pure Rust) ☑
  Caddy:    OpenSSL (C FFI) ☑
```

---

## Troubleshooting

### Backend not starting
```bash
# Check if port 9999 is in use
lsof -i :9999

# Kill any process using it
kill -9 <PID>

# Try again
bash scripts/start-app.sh
```

### Caddy.rs build fails
```bash
# Update Rust
rustup update

# Clean build
cargo clean
cargo build --release
```

### Port already in use
```bash
# Find process using port 80/443
lsof -i :80
lsof -i :443

# Kill it
kill -9 <PID>
```

### Apache Bench not found
```bash
# macOS
brew install httpd

# Linux (Ubuntu/Debian)
apt-get install apache2-utils

# Verify
ab -h
```

### Tests hang
```bash
# Check if backend is running
lsof -i :9999

# Force cleanup
pkill -f caddyrs
pkill -f caddy
pkill -f "http.server"
```

---

## Advanced Usage

### Run Tests with Custom Concurrency

Edit the test script and change:

```bash
# In test-concurrent-load.sh, modify TESTS variable:
TESTS=(
    "50:50:Custom - 50 requests, 50 concurrent"
    "500:200:Custom - 500 requests, 200 concurrent"
)
```

### Test with Different Backend

```bash
# Instead of Python HTTP server, run your app on :9999
your-app --port 9999

# Then run tests normally
bash scripts/test-basic-http.sh
```

### Monitor Test in Real Time

```bash
# Terminal 1: Backend
bash scripts/start-app.sh

# Terminal 2: Caddy.rs
target/release/caddyrs --config caddy.rs.toml

# Terminal 3: Monitor traffic
watch -n 1 'curl -s http://localhost:9090/metrics | grep http_requests'

# Terminal 4: Load test
ab -n 10000 -c 100 http://localhost:80/
```

### Save Test Results to CSV

```bash
# Extract startup times to CSV
cat test-results/caddy-rs-startup.txt test-results/caddy-startup.txt | \
  awk '{print $1}' | \
  paste -d',' test-results/caddy-rs-startup.txt test-results/caddy-startup.txt
```

---

## Next Steps

After comparing results:

### If caddy.rs is a good fit:
1. Review [README.md](README.md) for features and config
2. Read [TESTING.md](TESTING.md) for detailed analysis
3. Deploy to your environment

### If you need Caddy features:
1. Check the feature matrix in [README.md](README.md)
2. Auto-HTTPS, ACME, large plugin ecosystem
3. Caddy is the right choice for your use case

### For deeper analysis:
```bash
# View all logs
tail -f logs/*.log

# Check raw metrics
cat test-results/metrics-raw.txt

# Export results for analysis
cp test-results/* /path/to/analysis/
```

---

## Script Summary

| Script | Purpose | Time | Both/One |
|--------|---------|------|----------|
| `setup.sh` | Build & download | 2-3 min | Setup |
| `start-app.sh` | Test backend | Keep running | Backend |
| `test-basic-http.sh` | HTTP proxying | 30 sec | Both |
| `test-concurrent-load.sh` | Load testing | 2-3 min | Both |
| `test-https.sh` | TLS performance | 2 min | Both |
| `test-memory.sh` | Memory profiling | 3 min | Both |
| `test-startup.sh` | Startup time | 1 min | Both |
| `test-config-reload.sh` | Config reload | 30 sec | caddy.rs |
| `test-metrics.sh` | Prometheus metrics | 30 sec | caddy.rs |
| `run-all-tests.sh` | All tests | 10-15 min | Orchestrator |

---

## Questions?

- Check [TESTING.md](TESTING.md) for detailed test methodology
- Read [README.md](README.md) for feature overview
- See [caddy.rs.toml](caddy.rs.toml) for configuration options

Happy testing! 🚀
