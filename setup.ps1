# ###############################################################################
# Blink Engine Setup Script (PowerShell)
#
# Automated setup for the Blink Engine trading system on Windows
# ###############################################################################

$ErrorActionPreference = "Stop"

# Color functions
function Write-Info {
    Write-Host "ℹ $args" -ForegroundColor Cyan
}

function Write-Success {
    Write-Host "✓ $args" -ForegroundColor Green
}

function Write-Warning {
    Write-Host "⚠ $args" -ForegroundColor Yellow
}

function Write-Error {
    Write-Host "✗ $args" -ForegroundColor Red
}

# Header
Write-Host ""
Write-Host "╔════════════════════════════════════════════════════════╗" -ForegroundColor Cyan
Write-Host "║      Blink Engine Setup Script v1.0 (PowerShell)       ║" -ForegroundColor Cyan
Write-Host "╚════════════════════════════════════════════════════════╝" -ForegroundColor Cyan
Write-Host ""

# ###############################################################################
# 1. Check if already in blink-engine directory
# ###############################################################################

if (-not (Test-Path "Cargo.toml")) {
    Write-Warning "Not in blink-engine directory. Attempting to navigate..."

    if (Test-Path "blink-engine") {
        Set-Location "blink-engine"
        Write-Success "Navigated to blink-engine\"
    } else {
        Write-Error "Could not find blink-engine directory."
        Write-Info "Please run this script from the root of the Blink repository"
        exit 1
    }
}

# ###############################################################################
# 2. Check for Rust and Cargo
# ###############################################################################

Write-Info "Checking for Rust toolchain..."

try {
    $rustVersion = rustc --version
    Write-Success "Found: $rustVersion"
} catch {
    Write-Error "Rust is not installed."
    Write-Info "Please download and install from: https://rustup.rs/"
    Write-Info "Or use: choco install rust"
    exit 1
}

try {
    $cargoVersion = cargo --version
    Write-Success "Found: $cargoVersion"
} catch {
    Write-Error "Cargo is not installed."
    exit 1
}

# ###############################################################################
# 3. Check for .env file
# ###############################################################################

Write-Host ""
Write-Info "Checking environment configuration..."

if (Test-Path ".env") {
    Write-Success "Found .env file"
} else {
    if (Test-Path ".env.example") {
        Write-Info "Creating .env from .env.example..."
        Copy-Item ".env.example" ".env"
        Write-Success "Created .env file"
        Write-Warning "Please edit .env with your configuration:"
        Write-Host "  - CLOB_HOST"
        Write-Host "  - WS_URL"
        Write-Host "  - RN1_WALLET"
        Write-Host "  - MARKETS"
        Write-Host "  - Live trading credentials (if using LIVE_TRADING=true)"
    } else {
        Write-Error "Neither .env nor .env.example found"
        exit 1
    }
}

# ###############################################################################
# 4. Build the project
# ###############################################################################

Write-Host ""
Write-Info "Building Blink Engine..."
Write-Info "This may take several minutes on first build..."
Write-Host ""

try {
    cargo build --release
    Write-Success "Build completed successfully"
} catch {
    Write-Error "Build failed. Check output above for errors."
    exit 1
}

# ###############################################################################
# 5. Optional: Run tests
# ###############################################################################

Write-Host ""
$runTests = Read-Host "Run tests? (y/n)"

if ($runTests -eq "y" -or $runTests -eq "Y") {
    Write-Info "Running tests..."
    try {
        cargo test --release
        Write-Success "All tests passed"
    } catch {
        Write-Error "Some tests failed. Check output above."
        exit 1
    }
}

# ###############################################################################
# 6. Verify build artifacts
# ###############################################################################

Write-Host ""
Write-Info "Verifying build artifacts..."

if (Test-Path "target\release\engine.exe") {
    Write-Success "Engine binary built successfully"
} else {
    Write-Error "Engine binary not found after build"
    exit 1
}

if (Test-Path "target\release\market-scanner.exe") {
    Write-Success "Market scanner binary built successfully"
} else {
    Write-Warning "Market scanner binary not found"
}

# ###############################################################################
# 7. Summary and next steps
# ###############################################################################

Write-Host ""
Write-Success "Setup completed successfully!"
Write-Host ""
Write-Host "Next steps:" -ForegroundColor Cyan
Write-Host ""
Write-Host "1. Edit .env with your configuration:"
Write-Host "   notepad .env"
Write-Host ""
Write-Host "2. Discover markets (optional):"
Write-Host "   cargo run -p market-scanner"
Write-Host ""
Write-Host "3. Run in read-only mode (recommended first):"
Write-Host "   cargo run -p engine"
Write-Host ""
Write-Host "4. Run paper trading with dashboard:"
Write-Host "   `$env:PAPER_TRADING='true'; `$env:TUI='true'; cargo run -p engine"
Write-Host ""
Write-Host "5. For live trading (use with caution):"
Write-Host "   `$env:LIVE_TRADING='true'; cargo run --release -p engine"
Write-Host ""
Write-Warning "Important security notes:"
Write-Host "   * Never commit .env to version control"
Write-Host "   * Always test in read-only or paper mode first"
Write-Host "   * TRADING_ENABLED must be explicitly set to true"
Write-Host "   * Verify all risk parameters before enabling live trading"
Write-Host ""
Write-Host "Documentation: README.md"
Write-Host ""
