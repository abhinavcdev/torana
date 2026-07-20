# torana

[![CI](https://github.com/abhinavcdev/torana/actions/workflows/ci.yml/badge.svg)](https://github.com/abhinavcdev/torana/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Rust 1.75+](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](Cargo.toml)

A tiny reverse proxy written in Rust for edge, sidecar, and embedded use cases. Single static binary (~4.6 MB), millisecond startup, pure-Rust TLS via [rustls](https://github.com/rustls/rustls) — no OpenSSL, no runtime dependencies.

*torana* (तोरण) is the Indian ceremonial gateway arch — a small, sturdy structure everything passes through.

> **Status: early stage (v0.4).** The features listed below work and are tested, but this project is young and has not been hardened by production traffic. Read [What it doesn't do yet](#what-it-doesnt-do-yet) before deploying it anywhere that matters.

## What it does

- **HTTP/1.1 reverse proxying** with streaming request/response bodies (no buffering)
- **Host- and path-based routing** across multiple routes, first-match-wins, with a catch-all fallback
- **Upstream connection pooling** — keep-alive connections are reused across requests
- **Active health checks** — routes can probe upstreams on an interval; unhealthy upstreams are skipped
- **Retries** — `GET`/`HEAD`/`OPTIONS` requests with no body automatically retry a different upstream on connect failure (never retries requests that might have already been partially sent)
- **Sandboxed WASM plugins** *(opt-in, `--features plugins`)* — a request filter compiled to WASM runs fuel-limited in a real sandbox with no filesystem/network/process access
- **TLS termination** with rustls
- **Weighted round-robin load balancing** across multiple upstreams
- **Zero-downtime config reload** via `SIGHUP` — validated before swap; routes, upstreams, health checks, and plugins all rebuild atomically
- **Per-route total timeout** (default 30s) — hanging upstreams return `504` instead of stalling clients
- **Graceful shutdown** — SIGTERM/SIGINT stop accepting, drain in-flight requests (15s cap), then exit
- **Correct proxy header handling** — strips hop-by-hop headers, sets `X-Forwarded-For` / `X-Forwarded-Proto`
- **Prometheus metrics** (`http_requests_total`, `http_request_duration_seconds`, `upstream_connection_errors`) and a `/healthz` endpoint
- **Structured JSON logging** via `tracing`
- **Config validation at startup** — invalid configs exit non-zero; accepted-but-unimplemented fields log a warning instead of being silently ignored
- **Embeddable** — the routing/forwarding core is a separate crate ([`torana-core`](torana-core)) you can use inside your own hyper server, independent of torana's listener/signal machinery. See [Using it as a library](#using-it-as-a-library).

## What it doesn't do (yet)

Planned but **not implemented** — the config schema reserves fields for some of these, and torana will warn if you set them:

- HTTP/2 and HTTP/3
- TLS to upstreams (`https://` upstream addresses are rejected)
- Circuit breaking (retries and health checks exist; a breaker that ejects a flapping upstream faster does not, yet)
- Header rewriting, traffic mirroring, mTLS (`tls_client_ca`)
- Automatic HTTPS / ACME — if you want certificates managed for you, use [Caddy](https://caddyserver.com)

If you need these today, use Caddy, nginx, or Envoy. torana trades features for a small, auditable binary.

## Quick start

```bash
# Build (Rust 1.75+)
cargo build --release

# Minimal config
cat > torana.toml <<'EOF'
[global]
log_level = "info"
metrics_addr = "127.0.0.1:9090"

[[listener]]
addr = "127.0.0.1:8080"
protocol = "http"

[[route]]
name = "default"
upstream = [
  { addr = "http://127.0.0.1:3000", weight = 100 },
]
EOF

# Run
./target/release/torana --config torana.toml

# Reload config without dropping the listener
kill -HUP $(pgrep torana)
```

For HTTPS termination, add a listener with a certificate (generate a self-signed test pair with `scripts/gen-certs.sh`):

```toml
[[listener]]
addr = "127.0.0.1:8443"
protocol = "https"
tls_cert = "./certs/tls.crt"
tls_key = "./certs/tls.key"
```

### Multiple routes, health checks, and retries

```toml
[[route]]
name = "api"
host = "api.example.com"    # exact match, or "*.example.com" for a wildcard
path = "/v1"                # prefix match, on segment boundaries
upstream = [
  { addr = "http://127.0.0.1:3000" },
  { addr = "http://127.0.0.1:3001" },
]
retries = 2                 # GET/HEAD/OPTIONS with no body retry a dead upstream

[route.health_check]
path = "/healthz"           # probed on each upstream
interval = "10s"
timeout = "2s"

[[route]]
name = "default"            # no host/path constraint: catch-all, put it last
upstream = [{ addr = "http://127.0.0.1:4000" }]
```

Routes are matched in config order — put more specific routes first, catch-alls last.

## Sandboxed WASM plugins (opt-in)

```bash
cargo build --release --features plugins   # adds ~6 MB for the wasmtime runtime
```

```toml
[[route]]
name = "api"
plugin = "./plugins/auth-filter.wasm"
upstream = [{ addr = "http://127.0.0.1:3000" }]
```

A plugin is a WASM module exporting `memory`, `alloc(len: i32) -> i32`, and `on_request(method_ptr, method_len, path_ptr, path_len) -> i32` (`0` allows the request; `400..600` denies with that status; anything else denies with `403`). No host functions are exposed — a plugin cannot touch the filesystem, network, or process; wasmtime's sandbox is the entire security boundary, and execution is fuel-limited so a runaway plugin can't hang a request. See [torana-core/src/plugin.rs](torana-core/src/plugin.rs) for the full ABI and a working example compiled in the test suite.

This is why the feature is opt-in: wasmtime brings a Cranelift JIT that roughly doubles the binary. The default build stays small.

## Using it as a library

The proxy engine lives in a separate crate, [`torana-core`](torana-core), with none of the standalone binary's listener/signal machinery. Embed it inside a hyper server you already run:

```rust
use torana_core::{Config, ProxyEngine, Metrics};
use std::sync::Arc;

let engine = ProxyEngine::new(config, Arc::new(metrics));
engine.spawn_health_probers().await;

// inside your own service_fn:
let response = engine.handle(req, client_addr, "http").await?;
```

`Server` (used by the `torana` binary) is a thin wrapper around `ProxyEngine` that adds listener binding, SIGHUP reload, and graceful SIGTERM/SIGINT shutdown — read [torana-core/src/server.rs](torana-core/src/server.rs) if you want the full batteries-included behavior instead.

## Configuration reference

| Field | Status | Notes |
|---|---|---|
| `global.log_level` | ✅ | `error`, `warn`, `info`, `debug`, `trace` (default `info`) |
| `global.log_format` | ✅ | `json` (default) or anything else for plain text |
| `global.metrics_addr` | ✅ | Prometheus + `/healthz` endpoint (default `127.0.0.1:9090`) |
| `global.workers` | ⚠️ ignored | Tokio manages its own thread pool |
| `listener.addr` | ✅ | Must parse as a socket address, e.g. `0.0.0.0:443` |
| `listener.protocol` | ✅ | `http` or `https` |
| `listener.tls_cert` / `tls_key` | ✅ | PEM files, required for `https` |
| `listener.tls_client_ca` | ⚠️ ignored | mTLS not implemented |
| `route.name` | ✅ | Identifier used in logs and metrics |
| `route.host` | ✅ | Exact hostname match (port stripped); `*.example.com` matches subdomains; unset matches any host |
| `route.path` | ✅ | Path prefix match on segment boundaries; unset matches any path |
| `route.upstream[].addr` | ✅ | `http://host:port` only |
| `route.upstream[].weight` | ✅ | Relative weight (default 100) |
| `route.retries` | ✅ | Max attempts across upstreams for GET/HEAD/OPTIONS with no body (default: 2 if multiple upstreams, else 1) |
| `route.health_check.path/interval/timeout` | ✅ | Active probing; absent means always-healthy (today's default) |
| `route.plugin` | ✅ *(needs `--features plugins`)* | Path to a sandboxed WASM request filter |
| `route.timeout.total` | ✅ | e.g. `500ms`, `30s`, `5m` (default `30s`) |
| `route.timeout.connect` / `first_byte` | ⚠️ ignored | Only `total` is enforced |
| `route.when` | ⚠️ ignored | Reserved for a future CEL-based matcher; use `host`/`path` today |
| `route.mirror` | ⚠️ ignored | Traffic mirroring not implemented |
| `route.headers` | ⚠️ ignored | Header rewriting not implemented |

Fields marked ⚠️ parse without error (so configs stay forward-compatible) but log a warning at startup.

## Signals

| Signal | Behavior |
|---|---|
| `SIGHUP` | Reload and validate config; rebuilds routes, load balancers, health probers, and plugins atomically. On error, keeps the current config running |
| `SIGTERM` / `SIGINT` | Stop accepting, drain in-flight connections (up to 15s), exit |

## Observability

- `GET /metrics` on `global.metrics_addr` serves Prometheus text format
- `GET /healthz` on the same listener returns `200 ok` for liveness/readiness probes
- Logs are structured JSON on stdout by default; set `log_format` to anything else for human-readable output

## Development

```bash
cargo test --workspace                                  # ~1s, no root needed
cargo test -p torana-core --features plugins             # WASM sandbox tests (needs wasm32-unknown-unknown)
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p torana-core --all-targets --features plugins -- -D warnings
cargo fmt --all --check
```

Integration tests spawn real proxy processes against in-process backends on random ports, so they run in parallel and touch nothing outside the test.

- [Benchmarking](bench/README.md) — Docker harness comparing torana against Caddy and nginx, with sample results
- [v0.1 design notes](docs/plans/2026-04-11-torana-v0.1-implementation.md)

## Relationship to Caddy

torana is **not** affiliated with or a replacement for [Caddy](https://caddyserver.com). Caddy is a mature, batteries-included web server with automatic HTTPS and a large plugin ecosystem. torana explores a different corner of the design space: the smallest useful reverse proxy for containers and edge nodes, where a small static binary, fast cold start, and a genuinely sandboxed extension mechanism matter more than a large feature surface.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
