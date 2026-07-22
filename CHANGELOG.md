# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security

- `route.upstream[].weight` is now capped at 10,000. The load balancer
  expands weights (after GCD reduction) into a flat round-robin schedule
  sized by the largest weight, so an unbounded value could force an
  unbounded allocation at startup or reload.
- The Docker image now runs as a non-root user (`65532:65532`, the common
  "nonroot" numeric convention) instead of root, verified end-to-end
  against the existing bench harness.
- The metrics/`/healthz` listener's response write is now bounded by a
  5s timeout, matching the read side, so a client that stops reading
  can't hold the connection open indefinitely.
- Documented the trust boundary on `X-Forwarded-For` (torana appends to,
  rather than replaces, a client-supplied value — the same convention
  nginx and Caddy use — so an upstream trusting it for access control
  must read the last hop, not the first) and clarified that rotating a
  static `tls_cert`/`tls_key` pair requires a restart, since a running
  listener keeps the certificate it was bound with (ACME-managed
  listeners are unaffected — they renew internally).
- Release binaries now ship with a `.sha256` checksum file alongside
  each archive.
- Added `.github/dependabot.yml` (cargo, npm, github-actions) and enabled
  GitHub's Dependabot vulnerability alerts and automated security
  updates for this repository.

## [0.6.0] - 2026-07-21

### Added

- **HTTP/2 (client-facing)**: HTTPS listeners now negotiate h2 via ALPN
  (offered first, falling back to http/1.1) and serve it with hyper's h2
  server. The proxy-to-upstream leg always speaks HTTP/1.1 regardless of
  what the client negotiated — this proxy doesn't support HTTP/2 upstreams,
  and forwarding the client's negotiated version to an http/1.1-only
  upstream client would be (and, during development, briefly was) a
  `UserUnsupportedVersion` error. Not feature-gated: hyper already ships
  the h2 builder, so this is part of the default build.
- **Automatic HTTPS via ACME** (RFC 8555, TLS-ALPN-01), behind a new opt-in
  `acme` Cargo feature (off by default — adds ~1.6 MB via `rustls-acme`
  and its dependency tree, a second-generation rustls stack it carries
  internally). `listener.acme` replaces a static `tls_cert`/`tls_key` pair
  with `domains`, an optional `contact_emails` list, `cache_dir` for
  issued certs and account keys, and `directory_url`/`staging` to target
  Let's Encrypt's staging directory or a private/test ACME server.
  `ca_cert` lets the ACME client trust a private CA when connecting to the
  directory itself — for an internal ACME server, or a local test server
  like Pebble. Not combinable with `tls_client_ca` (mTLS) in this version.
  Certificate issuance and renewal run in a background task; the TLS
  handshake for real traffic completes once a certificate is available,
  handled through the same per-listener accept loop as static-cert and
  mTLS listeners.

### Fixed

- rustls 0.23 was refusing to auto-select a crypto provider at the first
  HTTPS handshake once a second dependency (rustls-acme, via
  `futures-rustls`) exercised the ambiguity: this crate's own `rustls`
  dependency never set `default-features = false`, so rustls's default
  feature set (pulling in `aws-lc-rs`) had been additively enabled
  alongside the explicit `ring` feature all along, without ever being
  triggered. Fixed at the source (`default-features = false` on the
  `rustls` dependency) plus a defensive explicit `install_default()` call
  at startup.

### Testing

Both new features are verified with genuine protocol-level end-to-end
tests, not synthetic load tools: an HTTP/2 test asserts the negotiated
protocol version via a real client and proxies several requests over one
pooled h2 connection to an http/1.1 backend; a same-listener HTTP/1.1
test proves the fallback path still works. The ACME feature is verified
against a local Pebble instance (Let's Encrypt's own ACME protocol test
server, run via Docker in `scripts/test-acme-e2e.sh`) — the test proves a
*genuine* RFC 8555 issuance occurred by verifying the served certificate's
chain against Pebble's actual issuing root, with a negative control
confirming the same request fails TLS verification without that trust
anchor. This also incidentally exercises Pebble's deliberately-injected
~5% bad-nonce rate, proving `rustls-acme`'s retry behavior recovers from
a real transient ACME protocol error, not just a happy path.

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
