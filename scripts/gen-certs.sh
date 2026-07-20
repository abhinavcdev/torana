#!/usr/bin/env bash
# Generate a self-signed certificate pair for local HTTPS testing.
set -euo pipefail
cd "$(dirname "$0")/.."

mkdir -p certs
if [ -f certs/tls.crt ] && [ -f certs/tls.key ]; then
  echo "certs/tls.crt and certs/tls.key already exist"
  exit 0
fi

openssl req -x509 -newkey rsa:4096 \
  -keyout certs/tls.key \
  -out certs/tls.crt \
  -days 365 -nodes \
  -subj "/CN=localhost" 2>/dev/null

echo "Wrote certs/tls.crt and certs/tls.key (self-signed, localhost, 365 days)"
