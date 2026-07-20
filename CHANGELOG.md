# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
