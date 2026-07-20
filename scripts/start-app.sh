#!/bin/bash

##########################################################################
# Start Test Application (Backend Server)
#
# Starts a Python HTTP server on port 9999 that serves as the upstream
# backend for both torana and Caddy to proxy to
##########################################################################

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
LOGS_DIR="$PROJECT_ROOT/logs"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

mkdir -p "$LOGS_DIR"

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}Starting Test Application (Backend)${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# Kill any existing test app
if lsof -i :9999 > /dev/null 2>&1; then
    echo -e "${YELLOW}Stopping existing backend on :9999...${NC}"
    lsof -i :9999 | awk 'NR!=1 {print $2}' | xargs kill -9 2>/dev/null || true
    sleep 1
fi

# Start test app
echo -e "${YELLOW}Starting Python HTTP server on port 9999...${NC}"
python3 -m http.server 9999 \
    --directory "$PROJECT_ROOT" \
    > "$LOGS_DIR/backend.log" 2>&1 &

APP_PID=$!
sleep 1

# Verify it started
if kill -0 $APP_PID 2>/dev/null; then
    echo -e "${GREEN}✓ Backend started (PID: $APP_PID)${NC}"
    echo ""
    echo "Logs:"
    echo -e "  ${YELLOW}tail -f $LOGS_DIR/backend.log${NC}"
    echo ""
    echo "Stop backend:"
    echo -e "  ${YELLOW}kill $APP_PID${NC}"
    echo ""

    # Wait indefinitely
    wait $APP_PID
else
    echo -e "${RED}✗ Failed to start backend${NC}"
    cat "$LOGS_DIR/backend.log"
    exit 1
fi
