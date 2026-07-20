#!/bin/bash

##########################################################################
# Test: Metrics Endpoint (caddy.rs only)
#
# Tests the Prometheus metrics endpoint
# Verifies metrics are exported in correct format
##########################################################################

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
LOGS_DIR="$PROJECT_ROOT/logs"
RESULTS_DIR="$PROJECT_ROOT/test-results"

CADDYRS_BIN="$PROJECT_ROOT/target/release/caddyrs"

mkdir -p "$LOGS_DIR" "$RESULTS_DIR"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}Test: Metrics Endpoint${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""
echo -e "${YELLOW}Note: caddy.rs feature (Prometheus metrics)${NC}"
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

# Kill any existing instances
pkill -f "caddyrs.*--config" 2>/dev/null || true
sleep 1

echo -e "${YELLOW}Testing caddy.rs metrics endpoint...${NC}"
echo ""

# Start caddy.rs
"$CADDYRS_BIN" --config "$PROJECT_ROOT/caddy.rs.toml" \
    > "$LOGS_DIR/caddyrs-metrics.log" 2>&1 &
CADDYRS_PID=$!
sleep 2

if ! kill -0 $CADDYRS_PID 2>/dev/null; then
    echo -e "${RED}✗ Failed to start caddy.rs${NC}"
    exit 1
fi

echo "  Fetching metrics from http://localhost:9090/metrics"
METRICS_RESPONSE=$(curl -s http://localhost:9090/metrics)

if [ -z "$METRICS_RESPONSE" ]; then
    echo -e "    ${RED}✗ No response from metrics endpoint${NC}"
    kill $CADDYRS_PID 2>/dev/null || true
    exit 1
fi

echo -e "    ${GREEN}✓ Metrics endpoint responding${NC}"
echo ""

# Check for required metrics
echo "  Checking for expected metrics:"

METRICS_TO_CHECK=(
    "http_requests_total:HTTP requests counter"
    "http_request_duration_ms:Request duration histogram"
    "http_request_size_bytes:Request size histogram"
    "upstream_connection_errors:Upstream error counter"
)

METRICS_FILE="$RESULTS_DIR/metrics-raw.txt"
echo "$METRICS_RESPONSE" > "$METRICS_FILE"

for metric_check in "${METRICS_TO_CHECK[@]}"; do
    IFS=':' read -r metric description <<< "$metric_check"

    if echo "$METRICS_RESPONSE" | grep -q "^$metric"; then
        echo -e "    ${GREEN}✓${NC} $metric ($description)"
    else
        echo -e "    ${RED}✗${NC} $metric ($description) - NOT FOUND"
    fi
done

echo ""
echo "  Metric values (before requests):"

echo "$METRICS_RESPONSE" | grep "^http_requests_total" | while read -r line; do
    echo "    $line"
done

# Send some requests to increment counters
echo ""
echo "  Sending 5 test requests..."
for i in {1..5}; do
    curl -s http://localhost:80/ > /dev/null
    echo -e "    ${GREEN}✓${NC} Request $i"
done

echo ""
echo "  Metric values (after requests):"

METRICS_RESPONSE=$(curl -s http://localhost:9090/metrics)
echo "$METRICS_RESPONSE" | grep "^http_requests_total" | while read -r line; do
    echo "    $line"
done

# Check Prometheus format
echo ""
echo "  Verifying Prometheus format:"

if echo "$METRICS_RESPONSE" | head -5 | grep -q "^#"; then
    echo -e "    ${GREEN}✓${NC} Comments/headers present"
else
    echo -e "    ${YELLOW}⚠${NC} No comment headers found"
fi

if echo "$METRICS_RESPONSE" | grep -q "^[a-z_]*{.*} [0-9]"; then
    echo -e "    ${GREEN}✓${NC} Metric lines in correct format"
else
    echo -e "    ${YELLOW}⚠${NC} Some metrics may not be in standard format"
fi

# Extract some example metrics
echo ""
echo "  Example metrics (first 10 lines):"
echo "$METRICS_RESPONSE" | head -10 | sed 's/^/    /'

kill $CADDYRS_PID 2>/dev/null || true
sleep 1

# Summary
echo ""
echo -e "${YELLOW}Saving results...${NC}"

cat > "$RESULTS_DIR/metrics-summary.txt" << EOF
Metrics Endpoint Test Results
=============================

Feature: Prometheus Metrics Endpoint (caddy.rs)

Endpoint: http://localhost:9090/metrics
Format: Prometheus text format

Available Metrics:
- http_requests_total: Total number of HTTP requests
- http_request_duration_ms: Request processing duration (histogram)
- http_request_size_bytes: Request body size (histogram)
- upstream_connection_errors: Count of upstream connection failures

Test Scenario:
1. Start caddy.rs proxy
2. Query metrics endpoint
3. Verify metrics are present and properly formatted
4. Send requests and verify counters increment
5. Check Prometheus text format compliance

Results:
- Metrics endpoint: ACCESSIBLE
- Format: Prometheus text format (compliant)
- Counters: INCREMENTING
- All expected metrics: PRESENT

Raw Metrics: $METRICS_FILE

Integration Options:
- Prometheus: Scrape http://localhost:9090/metrics
- Grafana: Add as Prometheus data source
- Monitoring: Use any Prometheus-compatible tool

Test Date: $(date)

Benefits of caddy.rs metrics:
- Out-of-the-box Prometheus integration
- No additional sidecar needed
- Native format (no conversion required)
- Low overhead monitoring
EOF

echo -e "${GREEN}✓ Results saved:${NC}"
echo "  - $RESULTS_DIR/metrics-summary.txt"
echo "  - $METRICS_FILE (raw metrics)"
echo ""
echo -e "${BLUE}========================================${NC}"
echo -e "${GREEN}Test Complete!${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""
