#!/bin/bash

##########################################################################
# Master Test Runner
#
# Runs all test scenarios in sequence with setup and teardown
##########################################################################

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${BLUE}"
echo "╔════════════════════════════════════════════════════╗"
echo "║                                                    ║"
echo "║         torana vs Caddy Testing Suite           ║"
echo "║                                                    ║"
echo "║     Complete Benchmarking and Comparison          ║"
echo "║                                                    ║"
echo "╚════════════════════════════════════════════════════╝"
echo -e "${NC}"
echo ""

# Step 1: Setup
echo -e "${YELLOW}Step 1: Setting up test environment...${NC}"
echo ""

if ! bash "$SCRIPT_DIR/setup.sh"; then
    echo -e "${RED}✗ Setup failed${NC}"
    exit 1
fi

echo ""
echo -e "${BLUE}========================================${NC}"
echo ""

# Step 2: Start test app
echo -e "${YELLOW}Step 2: Starting test application...${NC}"
echo ""

bash "$SCRIPT_DIR/start-app.sh" > /dev/null 2>&1 &
APP_PID=$!

sleep 3

if ! kill -0 $APP_PID 2>/dev/null; then
    echo -e "${RED}✗ Failed to start test application${NC}"
    exit 1
fi

echo -e "${GREEN}✓ Test application started${NC}"
echo ""

# Cleanup function
cleanup() {
    echo ""
    echo -e "${YELLOW}Cleaning up...${NC}"
    pkill -f "torana.*--config" 2>/dev/null || true
    pkill -f "caddy.*run.*--config" 2>/dev/null || true
    pkill -f "http.server" 2>/dev/null || true
    kill $APP_PID 2>/dev/null || true
    sleep 1
}

trap cleanup EXIT

# Step 3: Run all tests
echo -e "${BLUE}========================================${NC}"
echo ""
echo -e "${YELLOW}Step 3: Running all tests...${NC}"
echo ""

TESTS=(
    "test-basic-http.sh:Basic HTTP Proxying"
    "test-concurrent-load.sh:Concurrent Load Testing"
    "test-https.sh:HTTPS/TLS Performance"
    "test-memory.sh:Memory Usage"
    "test-startup.sh:Cold Startup Time"
    "test-config-reload.sh:Config Reload (torana only)"
    "test-metrics.sh:Metrics Endpoint (torana only)"
)

PASSED=0
FAILED=0

for test_config in "${TESTS[@]}"; do
    IFS=':' read -r test_script test_name <<< "$test_config"

    echo -e "${BLUE}────────────────────────────────────${NC}"
    echo ""

    if bash "$SCRIPT_DIR/$test_script"; then
        echo -e "${GREEN}✓ $test_name PASSED${NC}"
        ((PASSED++))
    else
        echo -e "${RED}✗ $test_name FAILED${NC}"
        ((FAILED++))
    fi

    echo ""
done

# Step 4: Summary
echo -e "${BLUE}════════════════════════════════════════════════════${NC}"
echo ""
echo -e "${BLUE}Test Summary${NC}"
echo -e "${BLUE}════════════════════════════════════════════════════${NC}"
echo ""
echo -e "  Passed: ${GREEN}$PASSED${NC}"
echo -e "  Failed: ${RED}$FAILED${NC}"
echo ""

if [ $FAILED -eq 0 ]; then
    echo -e "${GREEN}All tests passed!${NC}"
else
    echo -e "${RED}Some tests failed. Check logs in: $PROJECT_ROOT/logs${NC}"
fi

echo ""
echo -e "${BLUE}Results Location${NC}"
echo -e "${BLUE}════════════════════════════════════════════════════${NC}"
echo ""
echo "Test results saved in:"
echo -e "  ${YELLOW}$PROJECT_ROOT/test-results/${NC}"
echo ""
echo "Available results:"
ls -lh "$PROJECT_ROOT/test-results/" | tail -n +2 | awk '{print "  - " $NF}'
echo ""
echo "Detailed logs:"
echo -e "  ${YELLOW}$PROJECT_ROOT/logs/${NC}"
echo ""

echo -e "${BLUE}════════════════════════════════════════════════════${NC}"
echo ""
echo -e "${GREEN}Testing complete!${NC}"
echo ""
echo "Next steps:"
echo "  1. Review results in: $PROJECT_ROOT/test-results/"
echo "  2. Compare metrics: torana vs Caddy"
echo "  3. Read TESTING.md for detailed analysis"
echo ""
