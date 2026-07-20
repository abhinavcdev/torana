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

One run on an Apple-silicon Mac (Docker Desktop, 15s, 64 connections) — expect different absolute numbers on your machine:

| | torana 0.2 | Caddy 2 | nginx (stock) |
|---|---|---|---|
| Throughput | 5,321 qps | 4,825 qps | 1,375 qps |
| p50 / p99 | 11ms / 31ms | 13ms / 29ms | 51ms / 83ms |
| Idle memory | 4.4 MiB | 20 MiB | 9.6 MiB |
| Image size | 4.7 MB | 60 MB | 62 MB |

History on this setup: v0.1 (no pooling) measured ~1,060 qps with a 152ms p99; upstream keep-alive pooling brought throughput to Caddy parity, and disabling Nagle (TCP_NODELAY on both accepted and upstream sockets) brought the p99 tail from 58ms down to ~31ms — level with Caddy within run-to-run variance.

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
