# Local benchmarking

Compares torana against Caddy and nginx, each reverse-proxying the same nginx static backend, with [fortio](https://github.com/fortio/fortio) as the load generator. Everything runs inside one Docker network so no host port-forwarding skews the numbers.

## Run

```bash
./bench/bench.sh                      # 15s per target, 64 connections
DURATION=30s CONNS=128 ./bench/bench.sh
docker compose -f bench/docker-compose.yml down   # teardown
```

Reported per proxy: throughput (qps), p50/p99 latency, idle memory after load, and image size.

## Caveats

- **Docker Desktop on macOS runs in a VM.** Treat the numbers as a *relative* comparison between the proxies; absolute throughput/latency on Linux bare metal will be better.
- All three proxies are HTTP/1.1, no TLS, default settings — this measures the plain forwarding hot path only.
- torana and Caddy pool upstream connections; the nginx config here is stock (no `upstream` keepalive block), which is why default nginx trails on throughput.

## Sample results

Two consecutive runs on an Apple-silicon Mac (Docker Desktop, 15s, 64 connections), back to back with no code changes between them — included both, not cherry-picked, to show the actual run-to-run spread:

| | torana 0.5 (run A) | torana 0.5 (run B) | Caddy 2 | nginx (stock) |
|---|---|---|---|---|
| Throughput | 4,462 qps | 5,084 qps | 4,495–4,704 qps | 1,317–1,342 qps |
| p50 / p99 | 11ms / 50ms | 10ms / 34ms | 11–12ms / 34–41ms | 46–48ms / 107–155ms |
| Idle memory | 5.2 MiB | — | 21 MiB | 9.7 MiB |
| Image size | 4.9 MB | — | 60 MB | 62 MB |

torana and Caddy are close enough that either can lead on a given run — expect different absolute numbers on your machine, and don't read a single run as a verdict.

History on this setup: v0.1 (no pooling) measured ~1,060 qps with a 152ms p99; upstream keep-alive pooling brought throughput to Caddy parity; disabling Nagle (TCP_NODELAY on both accepted and upstream sockets) brought the p99 tail down to ~31ms; v0.4 added route matching, active health checks, and connect retries with no measurable regression; v0.5 added header rewriting, traffic mirroring, and mTLS — all three are no-ops on the request hot path unless a route actually configures them, and the bench config here doesn't, so the small idle-memory increase (4.8 → 5.2 MiB) is bookkeeping overhead, not per-request cost.

## Native alternative (no Docker)

More representative latency on the Mac itself:

```bash
brew install hey
cargo build --release
python3 -m http.server 9999 &            # backend
./target/release/torana --config torana.toml &   # proxies :8080 -> :9999
hey -z 15s -c 64 http://127.0.0.1:8080/
```

Startup time and binary size:

```bash
time ./target/release/torana --config nonexistent.toml   # ~cold start + config error
ls -lh target/release/torana
```
