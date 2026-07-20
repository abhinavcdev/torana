#!/bin/bash

##########################################################################
# caddy.rs Testing Setup Script
#
# Sets up both caddy.rs and Caddy for side-by-side comparison
# Downloads dependencies, generates test certs, and verifies setup
##########################################################################

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
TOOLS_DIR="$PROJECT_ROOT/tools"
CADDY_VERSION="2.11.2"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}caddy.rs Testing Setup${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# Create tools directory
mkdir -p "$TOOLS_DIR"
echo -e "${YELLOW}[1/5]${NC} Creating tools directory: $TOOLS_DIR"

# Build caddy.rs
echo ""
echo -e "${YELLOW}[2/5]${NC} Building caddy.rs..."
cd "$PROJECT_ROOT"

if command -v cargo &> /dev/null; then
    cargo build --release 2>&1 | grep -E "Finished|error" || true
    CADDYRS_BIN="$PROJECT_ROOT/target/release/caddyrs"
    if [ -f "$CADDYRS_BIN" ]; then
        echo -e "${GREEN}✓ caddy.rs built successfully${NC}"
        ls -lh "$CADDYRS_BIN"
    else
        echo -e "${RED}✗ Failed to build caddy.rs${NC}"
        exit 1
    fi
else
    echo -e "${RED}✗ Rust/Cargo not installed${NC}"
    exit 1
fi

# Build Caddy from source
echo ""
echo -e "${YELLOW}[3/5]${NC} Building Caddy from source..."
CADDY_BIN="$TOOLS_DIR/caddy"

if [ -f "$CADDY_BIN" ]; then
    echo -e "${GREEN}✓ Caddy already exists${NC}"
    "$CADDY_BIN" version
else
    # Check for Go
    if ! command -v go &> /dev/null; then
        echo -e "${RED}✗ Go not installed${NC}"
        echo "Install from: https://golang.org/dl/"
        exit 1
    fi

    CADDY_SRC="$TOOLS_DIR/caddy-src"

    # Clone Caddy repository
    echo "  Cloning Caddy repository..."
    if [ -d "$CADDY_SRC" ]; then
        echo "  (using existing clone)"
    else
        git clone --depth 1 --branch v2.7.6 https://github.com/caddyserver/caddy.git "$CADDY_SRC" 2>&1 | grep -E "Cloning|fatal" || true
    fi

    if [ ! -d "$CADDY_SRC" ]; then
        echo -e "${RED}✗ Failed to clone Caddy${NC}"
        exit 1
    fi

    # Build Caddy
    echo "  Building Caddy..."
    cd "$CADDY_SRC/cmd/caddy"
    go build -o "$CADDY_BIN" 2>&1 | grep -E "error|warning" || true

    if [ ! -f "$CADDY_BIN" ]; then
        echo -e "${RED}✗ Failed to build Caddy${NC}"
        exit 1
    fi

    chmod +x "$CADDY_BIN"
    echo -e "${GREEN}✓ Caddy built successfully${NC}"

    cd "$PROJECT_ROOT"
    "$CADDY_BIN" version
fi

# Generate test certificates
echo ""
echo -e "${YELLOW}[4/5]${NC} Generating test certificates..."
CERTS_DIR="$PROJECT_ROOT/certs"
mkdir -p "$CERTS_DIR"

if [ -f "$CERTS_DIR/tls.crt" ] && [ -f "$CERTS_DIR/tls.key" ]; then
    echo -e "${GREEN}✓ Test certificates already exist${NC}"
else
    if command -v openssl &> /dev/null; then
        openssl req -x509 -newkey rsa:4096 \
            -keyout "$CERTS_DIR/tls.key" \
            -out "$CERTS_DIR/tls.crt" \
            -days 365 -nodes \
            -subj "/CN=localhost" > /dev/null 2>&1
        echo -e "${GREEN}✓ Test certificates generated${NC}"
    else
        echo -e "${RED}✗ OpenSSL not installed${NC}"
        exit 1
    fi
fi

# Verify tools
echo ""
echo -e "${YELLOW}[5/5]${NC} Verifying tools and dependencies..."

TOOLS=(
    "curl:HTTP client"
    "ab:Apache Bench (load testing)"
    "wrk:HTTP benchmarking tool (optional)"
    "python3:Python runtime"
)

echo ""
for tool_check in "${TOOLS[@]}"; do
    IFS=':' read -r tool description <<< "$tool_check"
    if command -v "$tool" &> /dev/null; then
        echo -e "${GREEN}✓${NC} $tool - $description"
    else
        if [ "$tool" = "wrk" ]; then
            echo -e "${YELLOW}⚠${NC} $tool - $description (optional, install from https://github.com/wg/wrk)"
        else
            echo -e "${RED}✗${NC} $tool - $description (REQUIRED)"
        fi
    fi
done

# Summary
echo ""
echo -e "${BLUE}========================================${NC}"
echo -e "${GREEN}Setup Complete!${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""
echo "Available commands:"
echo ""
echo "  Start test app (backend):"
echo -e "    ${YELLOW}bash $SCRIPT_DIR/start-app.sh${NC}"
echo ""
echo "  Run individual tests:"
echo -e "    ${YELLOW}bash $SCRIPT_DIR/test-basic-http.sh${NC}"
echo -e "    ${YELLOW}bash $SCRIPT_DIR/test-concurrent-load.sh${NC}"
echo -e "    ${YELLOW}bash $SCRIPT_DIR/test-https.sh${NC}"
echo -e "    ${YELLOW}bash $SCRIPT_DIR/test-memory.sh${NC}"
echo -e "    ${YELLOW}bash $SCRIPT_DIR/test-startup.sh${NC}"
echo -e "    ${YELLOW}bash $SCRIPT_DIR/test-config-reload.sh${NC}"
echo -e "    ${YELLOW}bash $SCRIPT_DIR/test-metrics.sh${NC}"
echo ""
echo "  Run all tests:"
echo -e "    ${YELLOW}bash $SCRIPT_DIR/run-all-tests.sh${NC}"
echo ""
echo "Paths:"
echo -e "  caddy.rs: ${GREEN}$CADDYRS_BIN${NC}"
echo -e "  Caddy:    ${GREEN}$CADDY_BIN${NC}"
echo -e "  Certs:    ${GREEN}$CERTS_DIR${NC}"
echo ""
