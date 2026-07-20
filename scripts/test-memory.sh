#!/bin/bash

##########################################################################
# Test: Memory Usage Comparison
#
# Measures idle memory and memory under load
# Uses system tools to sample RSS memory
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
echo -e "${BLUE}Test: Memory Usage${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# Check prerequisites
if ! command -v ab &> /dev/null; then
    echo -e "${RED}✗ Apache Bench (ab) not found${NC}"
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

# Helper function to get RSS memory in MB
get_memory_mb() {
    local pid=$1
    if [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null; then
        echo "0"
        return
    fi

    if [ "$(uname)" = "Darwin" ]; then
        # macOS: memory in bytes, convert to MB
        ps -o rss= -p "$pid" | awk '{printf "%.1f", $1 / 1024}'
    else
        # Linux: memory in KB, convert to MB
        ps -o rss= -p "$pid" | awk '{printf "%.1f", $1 / 1024}'
    fi
}

echo -e "${YELLOW}Testing caddy.rs...${NC}"
echo ""

# Start caddy.rs
"$CADDYRS_BIN" --config "$PROJECT_ROOT/caddy.rs.toml" \
    > "$LOGS_DIR/caddyrs-memory.log" 2>&1 &
CADDYRS_PID=$!
sleep 2

if ! kill -0 $CADDYRS_PID 2>/dev/null; then
    echo -e "${RED}✗ Failed to start caddy.rs${NC}"
    exit 1
fi

echo "  Memory at idle (after 5 seconds):"
sleep 5
CADDYRS_IDLE=$(get_memory_mb $CADDYRS_PID)
echo -e "    ${GREEN}$CADDYRS_IDLE MB${NC}"

echo ""
echo "  Memory under load (1000 requests, 100 concurrent)..."
echo "    Sampling memory during load..."

# Start load in background and monitor memory
CADDYRS_LOAD_RESULTS="$RESULTS_DIR/caddy-rs-memory-load.txt"
> "$CADDYRS_LOAD_RESULTS"

# Sample memory every 0.5 seconds during load
(
    ab -n 1000 -c 100 -q http://localhost:80/ > /dev/null 2>&1 &
    LOAD_PID=$!

    while kill -0 $LOAD_PID 2>/dev/null; do
        MEM=$(get_memory_mb $CADDYRS_PID)
        echo "$(date '+%H:%M:%S.%N'): $MEM MB" >> "$CADDYRS_LOAD_RESULTS"
        sleep 0.5
    done

    wait $LOAD_PID
) &

sleep 8

CADDYRS_PEAK=$(tail -20 "$CADDYRS_LOAD_RESULTS" | awk '{print $NF}' | sort -nr | head -1)
echo -e "    ${GREEN}Peak: $CADDYRS_PEAK MB${NC}"

kill $CADDYRS_PID 2>/dev/null || true
sleep 1

echo ""
echo -e "${YELLOW}Testing Caddy...${NC}"
echo ""

# Start Caddy
"$CADDY_BIN" run --config "$PROJECT_ROOT/Caddyfile" \
    > "$LOGS_DIR/caddy-memory.log" 2>&1 &
CADDY_PID=$!
sleep 2

if ! kill -0 $CADDY_PID 2>/dev/null; then
    echo -e "${RED}✗ Failed to start Caddy${NC}"
    exit 1
fi

echo "  Memory at idle (after 5 seconds):"
sleep 5
CADDY_IDLE=$(get_memory_mb $CADDY_PID)
echo -e "    ${GREEN}$CADDY_IDLE MB${NC}"

echo ""
echo "  Memory under load (1000 requests, 100 concurrent)..."
echo "    Sampling memory during load..."

# Sample memory during load
CADDY_LOAD_RESULTS="$RESULTS_DIR/caddy-memory-load.txt"
> "$CADDY_LOAD_RESULTS"

(
    ab -n 1000 -c 100 -q http://localhost:80/ > /dev/null 2>&1 &
    LOAD_PID=$!

    while kill -0 $LOAD_PID 2>/dev/null; do
        MEM=$(get_memory_mb $CADDY_PID)
        echo "$(date '+%H:%M:%S.%N'): $MEM MB" >> "$CADDY_LOAD_RESULTS"
        sleep 0.5
    done

    wait $LOAD_PID
) &

sleep 8

CADDY_PEAK=$(tail -20 "$CADDY_LOAD_RESULTS" | awk '{print $NF}' | sort -nr | head -1)
echo -e "    ${GREEN}Peak: $CADDY_PEAK MB${NC}"

kill $CADDY_PID 2>/dev/null || true
sleep 1

# Comparison
echo ""
echo -e "${YELLOW}Comparison:${NC}"
echo ""

if command -v bc &> /dev/null; then
    RATIO=$(echo "scale=1; $CADDY_IDLE / $CADDYRS_IDLE" | bc)
    echo -e "  Idle Memory Ratio: ${GREEN}$RATIO x${NC} (Caddy uses $RATIO times more)"
fi

# Summary
echo ""
echo -e "${YELLOW}Saving results...${NC}"

cat > "$RESULTS_DIR/memory-summary.txt" << EOF
Memory Usage Test Results
==========================

Test Configuration:
- Load: 1000 requests, 100 concurrent
- Load duration: ~8 seconds

caddy.rs Memory:
- Idle (5s after startup): $CADDYRS_IDLE MB
- Peak under load: $CADDYRS_PEAK MB
- Detailed samples: $CADDYRS_LOAD_RESULTS

Caddy Memory:
- Idle (5s after startup): $CADDY_IDLE MB
- Peak under load: $CADDY_PEAK MB
- Detailed samples: $CADDY_LOAD_RESULTS

Comparison:
- Idle memory difference: $(echo "$CADDY_IDLE - $CADDYRS_IDLE" | bc) MB
- Caddy uses $(if command -v bc &> /dev/null; then echo "scale=1; $CADDY_IDLE / $CADDYRS_IDLE" | bc; else echo "approximately"; fi) times more memory at idle

Test Date: $(date)

Notes:
- Idle memory measured 5 seconds after startup
- Peak memory measured during 1000 request / 100 concurrent load test
- Memory sampling interval: 0.5 seconds
- See detailed samples in separate files for trends
EOF

echo -e "${GREEN}✓ Results saved:${NC}"
echo "  - $RESULTS_DIR/memory-summary.txt"
echo "  - $CADDYRS_LOAD_RESULTS (caddy.rs samples)"
echo "  - $CADDY_LOAD_RESULTS (Caddy samples)"
echo ""
echo -e "${BLUE}========================================${NC}"
echo -e "${GREEN}Test Complete!${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""
