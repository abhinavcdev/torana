#!/bin/bash

##########################################################################
# Test: Concurrent Load Testing
#
# Tests throughput and latency under concurrent load
# Uses Apache Bench (ab) for load generation
##########################################################################

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
TOOLS_DIR="$PROJECT_ROOT/tools"
LOGS_DIR="$PROJECT_ROOT/logs"
RESULTS_DIR="$PROJECT_ROOT/test-results"

CADDYRS_BIN="$PROJECT_ROOT/target/release/caddyrs"
CADDY_BIN="$TOOLS_DIR/caddy"

mkdir -p "$LOGS_DIR" "$RESULTS_DIR"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}Test: Concurrent Load${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# Check prerequisites
if ! command -v ab &> /dev/null; then
    echo -e "${RED}✗ Apache Bench (ab) not found${NC}"
    echo "Install with: brew install httpd (macOS) or apt-get install apache2-utils (Linux)"
    exit 1
fi

if ! lsof -i :9999 > /dev/null 2>&1; then
    echo -e "${RED}✗ Backend not running on :9999${NC}"
    echo "Start it with: bash $SCRIPT_DIR/start-app.sh"
    exit 1
fi

# Kill any existing instances
pkill -f "caddyrs.*--config" 2>/dev/null || true
pkill -f "caddy.*run.*--config" 2>/dev/null || true
sleep 1

# Create Caddyfile
cat > "$PROJECT_ROOT/Caddyfile" << 'EOF'
:80 {
    reverse_proxy localhost:9999
}
EOF

# Test configurations
TESTS=(
    "10:10:Light Load - 10 requests, 10 concurrent"
    "100:50:Medium Load - 100 requests, 50 concurrent"
    "1000:100:Heavy Load - 1000 requests, 100 concurrent"
)

echo -e "${YELLOW}Testing caddy.rs...${NC}"
echo ""

# Start caddy.rs
"$CADDYRS_BIN" --config "$PROJECT_ROOT/caddy.rs.toml" \
    > "$LOGS_DIR/caddyrs-concurrent.log" 2>&1 &
CADDYRS_PID=$!
sleep 2

if ! kill -0 $CADDYRS_PID 2>/dev/null; then
    echo -e "${RED}✗ Failed to start caddy.rs${NC}"
    exit 1
fi

CADDYRS_RESULTS="$RESULTS_DIR/caddy-rs-concurrent.txt"
> "$CADDYRS_RESULTS"

for test_config in "${TESTS[@]}"; do
    IFS=':' read -r requests concurrent description <<< "$test_config"
    echo -e "  ${YELLOW}$description${NC}"

    ab -n "$requests" -c "$concurrent" -q http://localhost:80/ 2>&1 | tee -a "$CADDYRS_RESULTS" | \
        grep -E "Requests per second|Concurrency Level|Completed" | sed 's/^/    /'

    echo ""
done

kill $CADDYRS_PID 2>/dev/null || true
sleep 1

echo -e "${YELLOW}Testing Caddy...${NC}"
echo ""

# Start Caddy
"$CADDY_BIN" run --config "$PROJECT_ROOT/Caddyfile" \
    > "$LOGS_DIR/caddy-concurrent.log" 2>&1 &
CADDY_PID=$!
sleep 2

if ! kill -0 $CADDY_PID 2>/dev/null; then
    echo -e "${RED}✗ Failed to start Caddy${NC}"
    exit 1
fi

CADDY_RESULTS="$RESULTS_DIR/caddy-concurrent.txt"
> "$CADDY_RESULTS"

for test_config in "${TESTS[@]}"; do
    IFS=':' read -r requests concurrent description <<< "$test_config"
    echo -e "  ${YELLOW}$description${NC}"

    ab -n "$requests" -c "$concurrent" -q http://localhost:80/ 2>&1 | tee -a "$CADDY_RESULTS" | \
        grep -E "Requests per second|Concurrency Level|Completed" | sed 's/^/    /'

    echo ""
done

kill $CADDY_PID 2>/dev/null || true
sleep 1

# Summary
echo -e "${YELLOW}Saving results...${NC}"

cat > "$RESULTS_DIR/concurrent-load-summary.txt" << EOF
Concurrent Load Test Results
=============================

Test Configurations:
1. Light Load: 10 requests, 10 concurrent
2. Medium Load: 100 requests, 50 concurrent
3. Heavy Load: 1000 requests, 100 concurrent

caddy.rs Results:
- Full results: $CADDYRS_RESULTS
- Logs: $LOGS_DIR/caddyrs-concurrent.log

Caddy Results:
- Full results: $CADDY_RESULTS
- Logs: $LOGS_DIR/caddy-concurrent.log

Test Date: $(date)

Note: For detailed results with latency analysis, see the individual result files.
Higher "Requests per second" is better.
EOF

echo -e "${GREEN}✓ Results saved:${NC}"
echo "  - $RESULTS_DIR/concurrent-load-summary.txt"
echo "  - $CADDYRS_RESULTS"
echo "  - $CADDY_RESULTS"
echo ""
echo -e "${BLUE}========================================${NC}"
echo -e "${GREEN}Test Complete!${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""
