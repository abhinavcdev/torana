#!/usr/bin/env bash
# Real end-to-end ACME test: starts Pebble (Let's Encrypt's own ACME
# protocol test server, not a mock), runs torana against it with the acme
# feature, and asserts a genuine RFC 8555 certificate was issued and served.
#
# Requires Docker. Not part of `cargo test` — this is the opt-in, slower,
# infrastructure-dependent counterpart, same relationship bench/bench.sh has
# to the default test suite.
set -euo pipefail
cd "$(dirname "$0")/.."

CONTAINER=torana-pebble-e2e

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "==> Starting Pebble (VA_ALWAYS_VALID: challenge network reachability isn't needed for this test)"
docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
docker run -d --name "$CONTAINER" \
  -p 14000:14000 -p 15000:15000 \
  -e PEBBLE_VA_ALWAYS_VALID=1 \
  -e PEBBLE_VA_NOSLEEP=1 \
  ghcr.io/letsencrypt/pebble:latest >/dev/null

echo "==> Waiting for Pebble's directory endpoint"
for _ in $(seq 1 30); do
  if curl -sk -o /dev/null "https://127.0.0.1:14000/dir"; then
    break
  fi
  sleep 1
done

echo "==> Running the real ACME issuance test"
cargo test -p torana --features acme --test acme_e2e -- --ignored --nocapture --test-threads=1

echo "==> Passed: torana obtained and served a real certificate from a real ACME server"
