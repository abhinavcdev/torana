# torana

A tiny reverse proxy written in Rust for edge, sidecar, and embedded use cases. Single static binary (~4 MB), millisecond startup, pure-Rust TLS via [rustls](https://github.com/rustls/rustls) — no OpenSSL, no runtime dependencies.

*torana* (तोरण) is the Indian ceremonial gateway arch — a small, sturdy structure everything passes through.

> **Status: early stage (v0.2).** The features listed below work and are tested, but this project is young and has not been hardened by production traffic. Read [What it doesn't do yet](#what-it-doesnt-do-yet) before deploying it anywhere that matters.

## What it does

- **HTTP/1.1 reverse proxying** with streaming request/response bodies (no buffering)
- **Upstream connection pooling** — keep-alive connections are reused across requests (90s idle timeout, up to 128 idle per upstream)
- **TLS termination** with rustls
- **Weighted round-robin load balancing** across multiple upstreams
- **Zero-downtime config reload** via `SIGHUP` — validated before swap, upstream changes apply immediately
- **Per-route total timeout** (default 30s) — hanging upstreams return `504` instead of stalling clients
- **Correct proxy header handling** — strips hop-by-hop headers, sets `X-Forwarded-For` / `X-Forwarded-Proto`
- **Prometheus metrics** (`http_requests_total`, `http_request_duration_seconds`, `upstream_connection_errors`)
- **Structured JSON logging** via `tracing`
- **Config validation at startup** — invalid configs exit non-zero; accepted-but-unimplemented fields log a warning instead of being silently ignored

## What it doesn't do (yet)

Planned but **not implemented** — the config schema reserves fields for some of these, and torana will warn if you set them:

- HTTP/2 and HTTP/3
- Request matching / multiple routes (all traffic currently goes to the first route)
- TLS to upstreams (`https://` upstream addresses are rejected)
- Retries, circuit breaking, active health checks
- Header rewriting, traffic mirroring, mTLS (`tls_client_ca`)
- Automatic HTTPS / ACME — if you want certificates managed for you, use [Caddy](https://caddyserver.com)
- Connection draining on shutdown

If you need these today, use Caddy, nginx, or Envoy. torana trades features for a small, auditable, dependency-free binary.

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

For HTTPS termination, add a listener with a certificate (generate a self-signed test pair with `scripts/setup.sh`):

```toml
[[listener]]
addr = "127.0.0.1:8443"
protocol = "https"
tls_cert = "./certs/tls.crt"
tls_key = "./certs/tls.key"
```

## Configuration reference

| Field | Status | Notes |
|---|---|---|
| `global.log_level` | ✅ | `error`, `warn`, `info`, `debug`, `trace` (default `info`) |
| `global.log_format` | ✅ | `json` (default) or anything else for plain text |
| `global.metrics_addr` | ✅ | Prometheus endpoint (default `127.0.0.1:9090`) |
| `global.workers` | ⚠️ ignored | Tokio manages its own thread pool |
| `listener.addr` | ✅ | Must parse as a socket address, e.g. `0.0.0.0:443` |
| `listener.protocol` | ✅ | `http` or `https` |
| `listener.tls_cert` / `tls_key` | ✅ | PEM files, required for `https` |
| `listener.tls_client_ca` | ⚠️ ignored | mTLS not implemented |
| `route.name` | ✅ | Identifier used in logs |
| `route.upstream[].addr` | ✅ | `http://host:port` only |
| `route.upstream[].weight` | ✅ | Relative weight (default 100) |
| `route.timeout.total` | ✅ | e.g. `500ms`, `30s`, `5m` (default `30s`) |
| `route.timeout.connect` / `first_byte` | ⚠️ ignored | Only `total` is enforced |
| `route.when` | ⚠️ ignored | Route matching not implemented; first route gets all traffic |
| `route.mirror` | ⚠️ ignored | Traffic mirroring not implemented |
| `route.headers` | ⚠️ ignored | Header rewriting not implemented |

Fields marked ⚠️ parse without error (so configs stay forward-compatible) but log a warning at startup.

## Signals

| Signal | Behavior |
|---|---|
| `SIGHUP` | Reload and validate config; on error, keep the current config |
| `SIGTERM` / `SIGINT` | Log and exit (in-flight connections are dropped — draining is future work) |

## Observability

- `GET /metrics` on `global.metrics_addr` serves Prometheus text format
- Logs are structured JSON on stdout by default; set `log_format` to anything else for human-readable output

## Development

```bash
cargo test          # unit + integration tests, no root needed, ~1s
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Integration tests spawn real proxy processes against in-process backends on random ports, so they run in parallel and touch nothing outside the test.

- [Local benchmarking guide](docs/TESTING.md) — side-by-side comparison scripts against Caddy
- [Scripts overview](docs/SCRIPTS_OVERVIEW.md)
- [v0.1 design notes](docs/plans/2026-04-11-torana-v0.1-implementation.md)

## Relationship to Caddy

torana is **not** affiliated with or a replacement for [Caddy](https://caddyserver.com). Caddy is a mature, batteries-included web server with automatic HTTPS and a large plugin ecosystem. torana explores a different corner of the design space: the smallest useful reverse proxy for containers and edge nodes, where a ~4 MB static binary and fast cold start matter more than features.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
