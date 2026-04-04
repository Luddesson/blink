#!/bin/bash

###############################################################################
#                    Blink Engine Setup Script                               #
#                                                                             #
# Automated setup for the Blink Engine trading system                        #
# Handles prerequisites, environment configuration, and build setup          #
###############################################################################

set -e

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Logging functions
log_info() {
    echo -e "${BLUE}ℹ${NC} $1"
}

log_success() {
    echo -e "${GREEN}✓${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}⚠${NC} $1"
}

log_error() {
    echo -e "${RED}✗${NC} $1"
}

###############################################################################
# Header
###############################################################################

echo ""
echo -e "${BLUE}╔════════════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║          Blink Engine Setup Script v1.0                ║${NC}"
echo -e "${BLUE}╚════════════════════════════════════════════════════════╝${NC}"
echo ""

###############################################################################
# 1. Check if already in blink-engine directory
###############################################################################

if [ ! -f "Cargo.toml" ]; then
    log_warn "Not in blink-engine directory. Attempting to navigate..."

    if [ -d "blink-engine" ]; then
        cd blink-engine
        log_success "Navigated to blink-engine/"
    else
        log_error "Could not find blink-engine directory."
        log_info "Please run this script from the root of the Blink repository or from blink-engine/"
        exit 1
    fi
fi

###############################################################################
# 2. Check OS
###############################################################################

log_info "Detecting operating system..."
OS_TYPE="unknown"

if [[ "$OSTYPE" == "linux-gnu"* ]]; then
    OS_TYPE="linux"
    log_success "Detected: Linux"
elif [[ "$OSTYPE" == "darwin"* ]]; then
    OS_TYPE="macos"
    log_success "Detected: macOS"
elif [[ "$OSTYPE" == "msys" ]] || [[ "$OSTYPE" == "cygwin" ]]; then
    OS_TYPE="windows"
    log_success "Detected: Windows"
else
    log_warn "Unknown OS: $OSTYPE"
    OS_TYPE="unknown"
fi

###############################################################################
# 3. Check for Rust and Cargo
###############################################################################

log_info "Checking for Rust toolchain..."

if ! command -v rustc &> /dev/null; then
    log_error "Rust is not installed."
    log_info "Installing Rust..."

    if [ "$OS_TYPE" = "windows" ]; then
        log_info "Please download and run the installer from: https://rustup.rs/"
        log_info "Or install using: choco install rust"
    else
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source $HOME/.cargo/env
    fi
fi

RUST_VERSION=$(rustc --version)
log_success "Found: $RUST_VERSION"

if ! command -v cargo &> /dev/null; then
    log_error "Cargo is not installed."
    exit 1
fi

CARGO_VERSION=$(cargo --version)
log_success "Found: $CARGO_VERSION"

###############################################################################
# 4. Check Rust version (1.78+)
###############################################################################

log_info "Verifying Rust version (requires 1.78+)..."

RUST_MINOR=$(rustc --version | sed -E 's/rustc 1\.([0-9]+).*/\1/')
if [ "$RUST_MINOR" -lt 78 ]; then
    log_warn "Rust version may be outdated. Updating toolchain..."
    rustup update stable
    log_success "Rust toolchain updated"
else
    log_success "Rust version is up to date"
fi

###############################################################################
# 5. Check for .env file
###############################################################################

log_info "Checking environment configuration..."

if [ -f ".env" ]; then
    log_success "Found .env file"
else
    if [ -f ".env.example" ]; then
        log_info "Creating .env from .env.example..."
        cp .env.example .env
        log_success "Created .env file"
        log_warn "Please edit .env with your configuration:"
        echo "  - CLOB_HOST"
        echo "  - WS_URL"
        echo "  - RN1_WALLET"
        echo "  - MARKETS"
        echo "  - Live trading credentials (if using LIVE_TRADING=true)"
    else
        log_error "Neither .env nor .env.example found"
        exit 1
    fi
fi

###############################################################################
# 6. Check for dependencies (system-level)
###############################################################################

log_info "Checking for system dependencies..."

if [ "$OS_TYPE" = "linux" ]; then
    if ! command -v pkg-config &> /dev/null; then
        log_warn "pkg-config not found. Installing..."
        if command -v apt &> /dev/null; then
            sudo apt-get update && sudo apt-get install -y pkg-config
        elif command -v yum &> /dev/null; then
            sudo yum install -y pkg-config
        else
            log_warn "Could not auto-install pkg-config. Please install manually."
        fi
    else
        log_success "pkg-config found"
    fi

    if ! command -v git &> /dev/null; then
        log_error "Git is required but not installed"
        exit 1
    fi
    log_success "git found"
fi

###############################################################################
# 7. Build the project
###############################################################################

log_info "Building Blink Engine..."
log_info "This may take several minutes on first build..."

if cargo build --release 2>&1; then
    log_success "Build completed successfully"
else
    log_error "Build failed. Check output above for errors."
    exit 1
fi

###############################################################################
# 8. Optional: Run tests
###############################################################################

echo ""
read -p "Run tests? (y/n) " -n 1 -r
echo ""

if [[ $REPLY =~ ^[Yy]$ ]]; then
    log_info "Running tests..."
    if cargo test --release 2>&1; then
        log_success "All tests passed"
    else
        log_error "Some tests failed. Check output above."
        exit 1
    fi
fi

###############################################################################
# 9. Verify build artifacts
###############################################################################

log_info "Verifying build artifacts..."

if [ -f "target/release/engine" ] || [ -f "target/release/engine.exe" ]; then
    log_success "Engine binary built successfully"
else
    log_error "Engine binary not found after build"
    exit 1
fi

if [ -f "target/release/market-scanner" ] || [ -f "target/release/market-scanner.exe" ]; then
    log_success "Market scanner binary built successfully"
else
    log_warn "Market scanner binary not found"
fi

###############################################################################
# 10. Summary and next steps
###############################################################################

echo ""
log_success "Setup completed successfully!"
echo ""
echo -e "${BLUE}Next steps:${NC}"
echo ""
echo "1. Edit .env with your configuration:"
echo "   nano .env"
echo ""
echo "2. Discover markets (optional):"
echo "   cargo run -p market-scanner"
echo ""
echo "3. Run in read-only mode (recommended first):"
echo "   cargo run -p engine"
echo ""
echo "4. Run paper trading with dashboard:"
echo "   PAPER_TRADING=true TUI=true cargo run -p engine"
echo ""
echo "5. For live trading (⚠️ use with caution):"
echo "   LIVE_TRADING=true cargo run --release -p engine"
echo ""
echo -e "${YELLOW}⚠  Important security notes:${NC}"
echo "   • Never commit .env to version control"
echo "   • Always test in read-only or paper mode first"
echo "   • TRADING_ENABLED must be explicitly set to true"
echo "   • Verify all risk parameters before enabling live trading"
echo ""
echo "Documentation: README.md"
echo ""
