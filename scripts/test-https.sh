#!/bin/bash

##########################################################################
# Test: HTTPS/TLS Performance
#
# Tests TLS handshake and HTTPS throughput
##########################################################################

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
TOOLS_DIR="$PROJECT_ROOT/tools"
LOGS_DIR="$PROJECT_ROOT/logs"
RESULTS_DIR="$PROJECT_ROOT/test-results"

CADDYRS_BIN="$PROJECT_ROOT/target/release/torana"
CADDY_BIN="$TOOLS_DIR/caddy"

mkdir -p "$LOGS_DIR" "$RESULTS_DIR"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}Test: HTTPS/TLS Performance${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# Check prerequisites
if ! command -v curl &> /dev/null; then
    echo -e "${RED}✗ curl not found${NC}"
    exit 1
fi

if ! command -v ab &> /dev/null; then
    echo -e "${RED}✗ Apache Bench (ab) not found${NC}"
    exit 1
fi

if ! lsof -i :9999 > /dev/null 2>&1; then
    echo -e "${RED}✗ Backend not running on :9999${NC}"
    echo "Start it with: bash $SCRIPT_DIR/start-app.sh"
    exit 1
fi

# Verify certificates exist
if [ ! -f "$PROJECT_ROOT/certs/tls.crt" ] || [ ! -f "$PROJECT_ROOT/certs/tls.key" ]; then
    echo -e "${RED}✗ TLS certificates not found${NC}"
    echo "Run setup: bash $SCRIPT_DIR/setup.sh"
    exit 1
fi

# Kill any existing instances
pkill -f "torana.*--config" 2>/dev/null || true
pkill -f "caddy.*run.*--config" 2>/dev/null || true
sleep 1

# Create Caddyfile
cat > "$PROJECT_ROOT/Caddyfile" << 'EOF'
:80 {
    reverse_proxy localhost:9999
}

:443 {
    tls ./certs/tls.crt ./certs/tls.key
    reverse_proxy localhost:9999
}
EOF

echo -e "${YELLOW}Testing torana...${NC}"
echo ""

# Start torana
"$CADDYRS_BIN" --config "$PROJECT_ROOT/torana.toml" \
    > "$LOGS_DIR/torana-https.log" 2>&1 &
CADDYRS_PID=$!
sleep 2

if ! kill -0 $CADDYRS_PID 2>/dev/null; then
    echo -e "${RED}✗ Failed to start torana${NC}"
    exit 1
fi

echo "  Testing HTTPS connectivity..."
RESPONSE=$(curl -s -k -w "\n%{http_code}" https://localhost:443/ 2>&1)
STATUS=$(echo "$RESPONSE" | tail -1)

if [ "$STATUS" = "200" ]; then
    echo -e "    ${GREEN}✓ HTTPS 200 OK${NC}"

    echo "  TLS version:"
    curl -s -k -v https://localhost:443/ 2>&1 | grep "SSL connection" | sed 's/^/    /'
else
    echo -e "    ${RED}✗ HTTPS $STATUS (expected 200)${NC}"
fi

echo ""
echo "  Load testing HTTPS (100 requests, 50 concurrent)..."
CADDYRS_HTTPS_RESULTS="$RESULTS_DIR/caddy-rs-https.txt"

# Note: Apache Bench with HTTPS and SSL verification disabled
ab -n 100 -c 50 -k https://localhost:443/ 2>&1 | tee "$CADDYRS_HTTPS_RESULTS" | \
    grep -E "Requests per second|Concurrency|Mean time|Transfer rate" | sed 's/^/    /'

kill $CADDYRS_PID 2>/dev/null || true
sleep 1

echo ""
echo -e "${YELLOW}Testing Caddy...${NC}"
echo ""

# Start Caddy with TLS
"$CADDY_BIN" run --config "$PROJECT_ROOT/Caddyfile" \
    > "$LOGS_DIR/caddy-https.log" 2>&1 &
CADDY_PID=$!
sleep 2

if ! kill -0 $CADDY_PID 2>/dev/null; then
    echo -e "${RED}✗ Failed to start Caddy${NC}"
    exit 1
fi

echo "  Testing HTTPS connectivity..."
RESPONSE=$(curl -s -k -w "\n%{http_code}" https://localhost:443/ 2>&1)
STATUS=$(echo "$RESPONSE" | tail -1)

if [ "$STATUS" = "200" ]; then
    echo -e "    ${GREEN}✓ HTTPS 200 OK${NC}"

    echo "  TLS version:"
    curl -s -k -v https://localhost:443/ 2>&1 | grep "SSL connection" | sed 's/^/    /'
else
    echo -e "    ${RED}✗ HTTPS $STATUS (expected 200)${NC}"
fi

echo ""
echo "  Load testing HTTPS (100 requests, 50 concurrent)..."
CADDY_HTTPS_RESULTS="$RESULTS_DIR/caddy-https.txt"

ab -n 100 -c 50 -k https://localhost:443/ 2>&1 | tee "$CADDY_HTTPS_RESULTS" | \
    grep -E "Requests per second|Concurrency|Mean time|Transfer rate" | sed 's/^/    /'

kill $CADDY_PID 2>/dev/null || true
sleep 1

# Summary
echo ""
echo -e "${YELLOW}Saving results...${NC}"

cat > "$RESULTS_DIR/https-summary.txt" << EOF
HTTPS/TLS Test Results
======================

Test Configuration:
- Protocol: HTTPS
- Load: 100 requests, 50 concurrent
- Keep-alive: Enabled
- Certificate: Self-signed (test cert)

torana Results:
- TLS Stack: rustls (pure Rust)
- Full results: $CADDYRS_HTTPS_RESULTS
- Logs: $LOGS_DIR/torana-https.log

Caddy Results:
- TLS Stack: OpenSSL (C FFI)
- Full results: $CADDY_HTTPS_RESULTS
- Logs: $LOGS_DIR/caddy-https.log

Test Date: $(date)

Key Metrics:
- Requests per second (higher is better)
- Mean time per request (lower is better)
- Transfer rate (throughput)

Note: TLS handshake overhead is significant. With connection keep-alive (-k flag),
handshake happens only once, so throughput should be similar to HTTP.
EOF

echo -e "${GREEN}✓ Results saved:${NC}"
echo "  - $RESULTS_DIR/https-summary.txt"
echo "  - $CADDYRS_HTTPS_RESULTS"
echo "  - $CADDY_HTTPS_RESULTS"
echo ""
echo -e "${BLUE}========================================${NC}"
echo -e "${GREEN}Test Complete!${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""
