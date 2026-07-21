# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-07-20

### Added

- **Header rewriting**: `[route.headers]` `request`/`response` maps are now
  enforced instead of warned-and-ignored. An empty value removes the
  header instead of setting it. Request overrides apply before the
  proxy's own `X-Forwarded-*` headers, so they can never be overridden
  by route config; response overrides apply after the upstream replies.
- **Traffic mirroring**: `route.mirror` fires a duplicate, fire-and-forget
  request at `mirror.addr` for a sampled fraction of traffic
  (`mirror.rate`, 0-100, default 100), without affecting the real
  response's latency or outcome. Only `GET`/`HEAD`/`OPTIONS` requests with
  no body are mirrored — the same body-replayability rule already used
  for retries, so a request is never duplicated in a way that could
  double-execute a side effect.
- **mTLS**: `listener.tls_client_ca` now enforces mandatory client
  certificate verification via rustls's `WebPkiClientVerifier` — a
  connection without a certificate signed by the given CA is rejected at
  the TLS handshake, before any HTTP is processed. On success, a SHA-256
  fingerprint of the verified client certificate is forwarded to the
  upstream as `X-Client-Cert-Fingerprint`; this header is always stripped
  from the client's original request first, so it can never be spoofed by
  a client on a plain HTTP/TLS connection.

## [0.4.0] - 2026-07-20

### Added

- **Host- and path-based routing**: `route.host` (exact or `*.` wildcard)
  and `route.path` (prefix, segment-boundary matched) let multiple routes
  coexist; first match in config order wins, with a catch-all fallback.
  Previously every request went to the first configured route.
- **Active health checks**: `[route.health_check]` probes each upstream on
  an interval; the load balancer skips upstreams the check considers down,
  and fails open (ignores health state) rather than blackholing all traffic
  if every upstream looks unhealthy at once.
- **Retries**: `route.retries` controls how many upstreams a `GET`/`HEAD`/
  `OPTIONS` request with no body will try before giving up. Only
  connect-level failures are retried, and only for requests with no body to
  replay — a request that reached an upstream is never retried, so
  non-idempotent requests are never double-executed.
- **Sandboxed WASM plugins** behind a new `plugins` Cargo feature (off by
  default — wasmtime adds a Cranelift JIT and roughly doubles the binary).
  `route.plugin` points at a WASM module that can allow or deny a request
  by method/path; no host functions are exposed, and execution is
  fuel-limited, so a plugin cannot access the filesystem, network, or
  process, and cannot hang a request by looping.
- **Embeddable core**: the engine now lives in a new `torana-core` library
  crate (`ProxyEngine`, usable inside your own hyper server) with `torana`
  reduced to a thin CLI wrapper (`Server`) around it. This is a workspace
  restructuring, not a behavior change, for the default binary.
- SIGHUP reload now rebuilds load balancers, health probers, and the
  plugin cache together, so a bad plugin or health-check config aborts the
  whole reload rather than half-applying it.

### Changed

- Upstream request bodies are boxed (`BoxBody<Bytes, hyper::Error>`)
  instead of the raw `hyper::body::Incoming` type, to allow constructing a
  fresh empty body per retry attempt without buffering real request
  bodies. Streaming behavior for non-retried requests is unchanged.

## [0.3.0] - 2026-07-20

### Added

- **Graceful shutdown with connection draining**: SIGTERM/SIGINT now stop
  accepting, let in-flight requests complete (15s cap), then exit — safe
  for rolling restarts behind an orchestrator.
- **`/healthz` endpoint** on the metrics listener for liveness and
  readiness probes.

### Changed

- Set `TCP_NODELAY` on accepted client sockets and upstream connections,
  cutting benchmark p99 latency roughly in half (Nagle + delayed-ACK
  interaction).
- Removed the legacy comparison scripts and docs superseded by `bench/`
  and `cargo test`; `scripts/gen-certs.sh` replaces `setup.sh` for local
  TLS material.
- Added binary release workflow (Linux musl + macOS, x86_64 + arm64).

## [0.2.0] - 2026-07-20

### Changed

- **Project renamed from caddyrs to torana.** The binary, default config
  file (`torana.toml`), and Docker paths all use the new name.

### Added

- **Upstream connection pooling**: keep-alive connections to upstreams are
  now reused across requests (hyper-util pooled client; 90s idle timeout,
  up to 128 idle connections per upstream). Previously every request opened
  a fresh TCP connection.

## [0.1.0] - 2026-07-19

Initial public release.

### Added

- HTTP/1.1 reverse proxying with streaming bodies
- TLS termination via rustls
- Weighted round-robin load balancing across upstreams
- Zero-downtime config reload on `SIGHUP` (validated before swap)
- Per-route total timeout (default 30s); hanging upstreams return 504
- Hop-by-hop header stripping, `X-Forwarded-For` / `X-Forwarded-Proto`
- Prometheus metrics endpoint and structured JSON logging
- Config validation at startup with warnings for unimplemented fields
