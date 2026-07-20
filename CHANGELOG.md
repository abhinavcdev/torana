# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
