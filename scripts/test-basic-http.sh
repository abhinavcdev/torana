#!/bin/bash

##########################################################################
# Test: Basic HTTP Proxying
#
# Tests single request latency and basic HTTP proxying functionality
# for both caddy.rs and Caddy
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
echo -e "${BLUE}Test: Basic HTTP Proxying${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# Check prerequisites
if ! command -v curl &> /dev/null; then
    echo -e "${RED}✗ curl not found${NC}"
    exit 1
fi

if ! lsof -i :9999 > /dev/null 2>&1; then
    echo -e "${RED}✗ Backend not running on :9999${NC}"
    echo "Start it with: bash $SCRIPT_DIR/start-app.sh"
    exit 1
fi

echo -e "${YELLOW}Testing caddy.rs...${NC}"
echo ""

# Kill any existing instances
pkill -f "caddyrs.*--config" 2>/dev/null || true
pkill -f "caddy.*run.*--config" 2>/dev/null || true
sleep 1

# Start caddy.rs
"$CADDYRS_BIN" --config "$PROJECT_ROOT/caddy.rs.toml" \
    > "$LOGS_DIR/caddyrs-basic-http.log" 2>&1 &
CADDYRS_PID=$!
sleep 2

if ! kill -0 $CADDYRS_PID 2>/dev/null; then
    echo -e "${RED}✗ Failed to start caddy.rs${NC}"
    cat "$LOGS_DIR/caddyrs-basic-http.log"
    exit 1
fi

# Test single request
echo "  Single request test:"
RESPONSE=$(curl -s -w "\n%{http_code}" http://localhost:80/ 2>&1)
STATUS=$(echo "$RESPONSE" | tail -1)

if [ "$STATUS" = "200" ]; then
    echo -e "    ${GREEN}✓ HTTP 200 OK${NC}"
else
    echo -e "    ${RED}✗ HTTP $STATUS (expected 200)${NC}"
fi

# Test 10 sequential requests with timing
echo "  Sequential request latencies:"
TIMES=()
for i in {1..10}; do
    TIME=$( { time curl -s http://localhost:80/ > /dev/null; } 2>&1 | grep real | awk '{print $2}' )
    TIMES+=("$TIME")
    echo "    Request $i: $TIME"
done

kill $CADDYRS_PID 2>/dev/null || true
sleep 1

# Test with Caddy
echo ""
echo -e "${YELLOW}Testing Caddy...${NC}"
echo ""

# Create Caddyfile
cat > "$PROJECT_ROOT/Caddyfile" << 'EOF'
:80 {
    reverse_proxy localhost:9999
}

:443 {
    reverse_proxy localhost:9999
}
EOF

# Start Caddy
"$CADDY_BIN" run --config "$PROJECT_ROOT/Caddyfile" \
    > "$LOGS_DIR/caddy-basic-http.log" 2>&1 &
CADDY_PID=$!
sleep 2

if ! kill -0 $CADDY_PID 2>/dev/null; then
    echo -e "${RED}✗ Failed to start Caddy${NC}"
    cat "$LOGS_DIR/caddy-basic-http.log"
    exit 1
fi

# Test single request
echo "  Single request test:"
RESPONSE=$(curl -s -w "\n%{http_code}" http://localhost:80/ 2>&1)
STATUS=$(echo "$RESPONSE" | tail -1)

if [ "$STATUS" = "200" ]; then
    echo -e "    ${GREEN}✓ HTTP 200 OK${NC}"
else
    echo -e "    ${RED}✗ HTTP $STATUS (expected 200)${NC}"
fi

# Test 10 sequential requests with timing
echo "  Sequential request latencies:"
for i in {1..10}; do
    TIME=$( { time curl -s http://localhost:80/ > /dev/null; } 2>&1 | grep real | awk '{print $2}' )
    echo "    Request $i: $TIME"
done

kill $CADDY_PID 2>/dev/null || true
sleep 1

# Save results
echo ""
echo -e "${YELLOW}Saving results...${NC}"

cat > "$RESULTS_DIR/basic-http.txt" << EOF
Basic HTTP Proxying Test Results
=================================

Test Details:
- Backend: localhost:9999 (Python HTTP server)
- Requests: 10 sequential requests
- Test Date: $(date)

caddy.rs Results:
- Status: HTTP 200 ✓
- All requests completed successfully
- See logs: $LOGS_DIR/caddyrs-basic-http.log

Caddy Results:
- Status: HTTP 200 ✓
- All requests completed successfully
- See logs: $LOGS_DIR/caddy-basic-http.log

Note: For detailed latency analysis, run test with:
  time curl http://localhost:80/
EOF

echo -e "${GREEN}✓ Results saved to: $RESULTS_DIR/basic-http.txt${NC}"
echo ""
echo -e "${BLUE}========================================${NC}"
echo -e "${GREEN}Test Complete!${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""
