#!/usr/bin/env bash
# Benchmark caddyrs against Caddy and nginx in Docker.
#
#   ./bench/bench.sh              # 15s per target, 64 connections
#   DURATION=30s CONNS=128 ./bench/bench.sh
#
# All proxies and the load generator run inside the same Docker network, so
# results are a fair relative comparison (absolute numbers include Docker
# Desktop VM overhead and will be lower than bare metal).

set -euo pipefail
cd "$(dirname "$0")"

DURATION="${DURATION:-15s}"
CONNS="${CONNS:-64}"
TARGETS=(caddyrs caddy nginx-proxy)

echo "==> Building and starting proxies (first caddyrs build takes a few minutes)"
docker compose up -d --build backend "${TARGETS[@]}"

echo "==> Waiting for proxies to answer"
for target in "${TARGETS[@]}"; do
  for _ in $(seq 1 30); do
    if docker compose run --rm fortio curl -quiet "http://${target}:8080/" >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done
done

echo "==> Warmup (5s each)"
for target in "${TARGETS[@]}"; do
  docker compose run --rm fortio load -quiet -c "$CONNS" -qps -1 -t 5s "http://${target}:8080/" >/dev/null
done

RESULTS=""
for target in "${TARGETS[@]}"; do
  echo ""
  echo "==> Benchmarking ${target} (${DURATION}, ${CONNS} connections, max qps)"
  OUT=$(docker compose run --rm fortio load -quiet -c "$CONNS" -qps -1 -t "$DURATION" \
    "http://${target}:8080/" 2>&1)
  echo "$OUT" | grep -E "target 50%|target 90%|target 99%|Aggregated|All done"
  QPS=$(echo "$OUT" | grep "All done" | sed -E 's/.* ([0-9.]+) qps.*/\1/')
  P50=$(echo "$OUT" | grep "target 50%" | head -1 | awk '{print $NF}')
  P99=$(echo "$OUT" | grep "target 99%" | head -1 | awk '{print $NF}')
  RESULTS+=$(printf "%-14s %10s qps   p50 %ss   p99 %ss" "$target" "$QPS" "$P50" "$P99")$'\n'
done

echo ""
echo "==> Idle memory after load (docker stats)"
sleep 2
docker stats --no-stream --format "table {{.Name}}\t{{.MemUsage}}" \
  $(docker compose ps -q "${TARGETS[@]}")

echo ""
echo "==> Image sizes"
docker images --format "table {{.Repository}}\t{{.Size}}" | grep -E "bench|caddy|nginx" | head -6

echo ""
echo "================ SUMMARY ================"
printf "%s" "$RESULTS"
echo "========================================="
echo ""
echo "Teardown: docker compose -f bench/docker-compose.yml down"
