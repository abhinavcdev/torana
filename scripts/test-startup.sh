#!/bin/bash

##########################################################################
# Test: Cold Startup Time
#
# Measures time from process start to accepting connections
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
echo -e "${BLUE}Test: Cold Startup Time${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# Check prerequisites
if ! command -v curl &> /dev/null; then
    echo -e "${RED}✗ curl not found${NC}"
    exit 1
fi

# Kill any existing instances
pkill -f "torana.*--config" 2>/dev/null || true
pkill -f "caddy.*run.*--config" 2>/dev/null || true
sleep 2

# Create Caddyfile
cat > "$PROJECT_ROOT/Caddyfile" << 'EOF'
:80 {
    reverse_proxy localhost:9999
}
EOF

# Helper function to measure startup time
measure_startup() {
    local binary=$1
    local config=$2
    local name=$3

    # Use perl for microsecond precision timing (works on macOS and Linux)
    local start_time=$(perl -e 'print int(time * 1000000)')

    if [ "$name" = "torana" ]; then
        "$binary" --config "$config" > /dev/null 2>&1 &
    else
        "$binary" run --config "$config" > /dev/null 2>&1 &
    fi
    PID=$!

    # Wait for process to be ready (accepting connections)
    local max_wait=10000  # 10 seconds in milliseconds
    local elapsed=0

    while [ $elapsed -lt $max_wait ]; do
        if curl -s http://localhost:80/ > /dev/null 2>&1; then
            local end_time=$(perl -e 'print int(time * 1000000)')
            local startup_us=$(( end_time - start_time ))
            local startup_ms=$(( (startup_us + 500) / 1000 ))  # Convert to ms with rounding
            echo "$startup_ms"
            kill $PID 2>/dev/null || true
            sleep 1
            return 0
        fi

        sleep 0.05
        elapsed=$((elapsed + 50))
    done

    echo "TIMEOUT"
    kill $PID 2>/dev/null || true
    return 1
}

# Test torana multiple times
echo -e "${YELLOW}Testing torana startup time...${NC}"
echo ""

CADDYRS_STARTUP_RESULTS="$RESULTS_DIR/caddy-rs-startup.txt"
> "$CADDYRS_STARTUP_RESULTS"

echo "  Running 5 startup tests:"
CADDYRS_TIMES=()

for i in {1..5}; do
    echo -n "    Attempt $i: "
    TIME=$(measure_startup "$CADDYRS_BIN" "$PROJECT_ROOT/torana.toml" "torana")

    if [ "$TIME" != "TIMEOUT" ]; then
        echo -e "${GREEN}${TIME}ms${NC}"
        CADDYRS_TIMES+=("$TIME")
        echo "$TIME" >> "$CADDYRS_STARTUP_RESULTS"
    else
        echo -e "${RED}TIMEOUT${NC}"
    fi

    sleep 1
done

# Calculate average for torana
if [ ${#CADDYRS_TIMES[@]} -gt 0 ]; then
    CADDYRS_AVG=$(printf '%s\n' "${CADDYRS_TIMES[@]}" | awk '{sum+=$1} END {printf "%.1f", sum/NR}')
    CADDYRS_MIN=$(printf '%s\n' "${CADDYRS_TIMES[@]}" | sort -n | head -1)
    CADDYRS_MAX=$(printf '%s\n' "${CADDYRS_TIMES[@]}" | sort -n | tail -1)
else
    CADDYRS_AVG="N/A"
    CADDYRS_MIN="N/A"
    CADDYRS_MAX="N/A"
fi

echo ""
echo -e "  torana Results:"
echo -e "    Average: ${GREEN}$CADDYRS_AVG ms${NC}"
echo -e "    Min: ${GREEN}$CADDYRS_MIN ms${NC}"
echo -e "    Max: ${GREEN}$CADDYRS_MAX ms${NC}"

sleep 2

# Test Caddy multiple times
echo ""
echo -e "${YELLOW}Testing Caddy startup time...${NC}"
echo ""

CADDY_STARTUP_RESULTS="$RESULTS_DIR/caddy-startup.txt"
> "$CADDY_STARTUP_RESULTS"

echo "  Running 5 startup tests:"
CADDY_TIMES=()

for i in {1..5}; do
    echo -n "    Attempt $i: "
    TIME=$(measure_startup "$CADDY_BIN" "$PROJECT_ROOT/Caddyfile" "caddy")

    if [ "$TIME" != "TIMEOUT" ]; then
        echo -e "${GREEN}${TIME}ms${NC}"
        CADDY_TIMES+=("$TIME")
        echo "$TIME" >> "$CADDY_STARTUP_RESULTS"
    else
        echo -e "${RED}TIMEOUT${NC}"
    fi

    sleep 1
done

# Calculate average for Caddy
if [ ${#CADDY_TIMES[@]} -gt 0 ]; then
    CADDY_AVG=$(printf '%s\n' "${CADDY_TIMES[@]}" | awk '{sum+=$1} END {printf "%.1f", sum/NR}')
    CADDY_MIN=$(printf '%s\n' "${CADDY_TIMES[@]}" | sort -n | head -1)
    CADDY_MAX=$(printf '%s\n' "${CADDY_TIMES[@]}" | sort -n | tail -1)
else
    CADDY_AVG="N/A"
    CADDY_MIN="N/A"
    CADDY_MAX="N/A"
fi

echo ""
echo -e "  Caddy Results:"
echo -e "    Average: ${GREEN}$CADDY_AVG ms${NC}"
echo -e "    Min: ${GREEN}$CADDY_MIN ms${NC}"
echo -e "    Max: ${GREEN}$CADDY_MAX ms${NC}"

# Comparison
echo ""
echo -e "${YELLOW}Comparison:${NC}"
echo ""

if command -v bc &> /dev/null && [ "$CADDYRS_AVG" != "N/A" ] && [ "$CADDY_AVG" != "N/A" ] && [ "${CADDYRS_AVG%.*}" != "0" ]; then
    RATIO=$(echo "scale=1; $CADDY_AVG / $CADDYRS_AVG" | bc)
    echo -e "  torana is ${GREEN}${RATIO}x faster${NC} than Caddy"
elif [ "$CADDYRS_AVG" != "N/A" ] && [ "$CADDY_AVG" != "N/A" ]; then
    echo -e "  Both startup times are extremely fast (<< 1ms)"
fi

# Summary
echo ""
echo -e "${YELLOW}Saving results...${NC}"

cat > "$RESULTS_DIR/startup-summary.txt" << EOF
Cold Startup Time Test Results
===============================

Test Configuration:
- Runs: 5 iterations per proxy
- Metric: Time from process start to accepting HTTP connections
- Measurement: Using curl to detect readiness

torana Results:
- Average: $CADDYRS_AVG ms
- Minimum: $CADDYRS_MIN ms
- Maximum: $CADDYRS_MAX ms
- Detailed results: $CADDYRS_STARTUP_RESULTS

Caddy Results:
- Average: $CADDY_AVG ms
- Minimum: $CADDY_MIN ms
- Maximum: $CADDY_MAX ms
- Detailed results: $CADDY_STARTUP_RESULTS

Comparison:
- Difference: $(echo "$CADDY_AVG - $CADDYRS_AVG" | bc) ms
- Speedup: $(if command -v bc &> /dev/null; then echo "scale=1; $CADDY_AVG / $CADDYRS_AVG" | bc; else echo "N/A"; fi)x faster

Test Date: $(date)

Analysis:
- Lower is better
- torana prioritizes fast startup (important for edge/serverless)
- Caddy has more startup overhead due to Go runtime initialization
EOF

echo -e "${GREEN}✓ Results saved:${NC}"
echo "  - $RESULTS_DIR/startup-summary.txt"
echo "  - $CADDYRS_STARTUP_RESULTS"
echo "  - $CADDY_STARTUP_RESULTS"
echo ""
echo -e "${BLUE}========================================${NC}"
echo -e "${GREEN}Test Complete!${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""
