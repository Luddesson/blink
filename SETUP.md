# Blink Engine Setup Guide

This guide will help you set up the Blink Engine trading system for Polymarket. Choose the appropriate setup method for your operating system.

**Table of Contents:**
- [Quick Start](#quick-start)
- [Linux/macOS Setup](#linuxmacos-setup)
- [Windows Setup](#windows-setup)
- [Manual Setup](#manual-setup)
- [Configuration](#configuration)
- [Verification](#verification)
- [Common Issues](#common-issues)

---

## Quick Start

### Linux/macOS
```bash
cd blink-engine
bash ../setup.sh
```

### Windows (PowerShell)
```powershell
Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope Process
.\setup.ps1
```

### Windows (Command Prompt)
```cmd
setup.bat
```

### Using Make (Linux/macOS)
```bash
make setup
```

---

## Linux/macOS Setup

### Option 1: Using the Bash Setup Script (Recommended)

The `setup.sh` script automates everything:

```bash
cd blink-engine
bash ../setup.sh
```

**What it does:**
- ✓ Checks for Rust and Cargo
- ✓ Installs Rust if needed
- ✓ Creates `.env` from `.env.example`
- ✓ Builds the project
- ✓ Optionally runs tests
- ✓ Verifies all binaries

### Option 2: Using Make

```bash
# Run complete setup
make setup

# Or check prerequisites first
make setup-check

# Then build
make build-release
```

### Option 3: Manual Setup

```bash
# 1. Install Rust if not already installed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# 2. Navigate to project
cd blink-engine

# 3. Create environment file
cp .env.example .env
nano .env  # Edit with your configuration

# 4. Build
cargo build --release

# 5. Run tests (optional)
cargo test
```

---

## Windows Setup

### Option 1: Using PowerShell (Recommended)

```powershell
# Allow script execution (one-time)
Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope Process

# Run setup
.\setup.ps1
```

### Option 2: Using Batch Script

```cmd
setup.bat
```

### Option 3: Manual Setup

```cmd
# 1. Install Rust from https://rustup.rs/
# Or using Chocolatey:
choco install rust

# 2. Open Command Prompt and navigate
cd blink-engine

# 3. Create environment file
copy .env.example .env
notepad .env

# 4. Build
cargo build --release

# 5. Run tests (optional)
cargo test
```

---

## Manual Setup

If the automated scripts don't work for you, follow these steps:

### 1. Install Rust

**Linux/macOS:**
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source $HOME/.cargo/env
```

**Windows:**
- Download from https://rustup.rs/
- Or use: `choco install rust`

Verify installation:
```bash
rustc --version
cargo --version
```

### 2. Clone/Navigate Repository

```bash
# If cloning fresh
git clone <repo-url>
cd blink-engine

# Or if already cloned
cd path/to/blink-engine
```

### 3. Create Environment File

```bash
cp .env.example .env
```

Then edit `.env` with your preferred editor. See [Configuration](#configuration) section below.

### 4. Build Project

```bash
# Debug build (faster compile)
cargo build

# Release build (optimized, recommended)
cargo build --release
```

### 5. Run Tests (Optional)

```bash
cargo test --release
```

---

## Configuration

### Create .env File

Copy the example and fill in your values:

```bash
cp .env.example .env
```

### Essential Configuration

#### Core Settings (Required)

| Variable | Description | Example |
|----------|-------------|---------|
| `CLOB_HOST` | Polymarket CLOB API base URL | `https://clob.polymarket.com` |
| `WS_URL` | WebSocket feed URL | `wss://ws-live-data.polymarket.com` |
| `RN1_WALLET` | Wallet to track (lowercase hex) | `0xabcd1234...` |
| `MARKETS` | Comma-separated market token IDs | `0x1234,0x5678` |

#### Trading Modes

| Variable | Default | Usage |
|----------|---------|-------|
| `PAPER_TRADING` | `false` | Set to `true` for simulation mode |
| `TUI` | `false` | Set to `true` with PAPER_TRADING for dashboard |
| `LIVE_TRADING` | `false` | Set to `true` for real trading (⚠️) |

#### Live Trading Credentials (Required if LIVE_TRADING=true)

```bash
SIGNER_PRIVATE_KEY=<your-64-char-hex-key>
POLYMARKET_FUNDER_ADDRESS=0x...
POLYMARKET_API_KEY=<your-api-key>
POLYMARKET_API_SECRET=<base64-encoded-secret>
POLYMARKET_API_PASSPHRASE=<your-passphrase>
POLYMARKET_SIGNATURE_TYPE=0
BLINK_LIVE_PROFILE=canonical-v1
```

#### Risk Management (Recommended)

| Variable | Default | Description |
|----------|---------|-------------|
| `MAX_DAILY_LOSS_PCT` | `0.10` | Maximum daily loss (10% of NAV) |
| `MAX_CONCURRENT_POSITIONS` | `5` | Max open positions simultaneously |
| `MAX_SINGLE_ORDER_USDC` | `20.0` | Max USDC per order |
| `MAX_ORDERS_PER_SECOND` | `3` | Rate limit for order submission |
| `TRADING_ENABLED` | `false` | Master kill switch (set to `true` to enable) |

### Discover Markets

Use the market scanner to find active markets:

```bash
cargo run -p market-scanner
```

This will show top markets by 24h volume and can auto-update your `.env`.

---

## Verification

After setup completes, verify everything works:

### Check Installation

```bash
# Using make
make setup-check

# Or manually
rustc --version
cargo --version
cd blink-engine && cargo check
```

### Verify Build Artifacts

```bash
# After build, check for binaries:
ls -la target/release/engine
ls -la target/release/market-scanner
```

### Test in Read-Only Mode

```bash
# Using make
make run

# Or manually
cd blink-engine && cargo run -p engine
```

You should see:
- ✓ WebSocket connection established
- ✓ Order book updates flowing
- ✓ Log output showing trading activity

---

## Running the Engine

### 1. Read-Only Mode (Recommended First)

Watch for RN1 signals without placing any orders:

```bash
# Using make
make run

# Or manually
cd blink-engine && cargo run -p engine
```

### 2. Paper Trading Mode

Simulate trading with $100 virtual USDC:

```bash
# Using make
make run-paper

# Or manually
cd blink-engine && PAPER_TRADING=true TUI=true cargo run -p engine
```

Features:
- Full terminal dashboard (ratatui TUI)
- Real-time P&L tracking
- Risk checks and position management
- No real funds used

### 3. Live Trading Mode ⚠️

**ONLY after extensive testing in paper mode!**

```bash
# Using make
make run-live

# Or manually
cd blink-engine && LIVE_TRADING=true cargo run --release -p engine
```

**Safety checks before proceeding:**
- [ ] Tested thoroughly in paper mode
- [ ] Verified all risk parameters in .env
- [ ] Confirmed TRADING_ENABLED=true
- [ ] All live credentials configured
- [ ] Understand the trading strategy

---

## Using Make Commands

The `Makefile` provides convenient shortcuts:

```bash
# Setup & Build
make setup              # Complete setup
make build              # Debug build
make build-release      # Optimized build

# Testing
make test               # Run tests
make check              # Quick check

# Running
make run                # Read-only mode
make run-paper          # Paper trading with TUI
make run-market-scanner # Discover markets

# Development
make lint               # Check code with clippy
make fmt                # Format code
make fmt-check          # Check formatting

# Logs
make logs               # Show log files
make logs-tail          # Follow latest log

# Show all options
make help
```

---

## Common Issues

### Issue: "rustc: command not found"

**Solution:** Install Rust
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

### Issue: "Could not find blink-engine directory"

**Solution:** Run the script from the correct location
```bash
# Should be in the root directory with both:
# - blink-engine/
# - setup.sh

# Or navigate into blink-engine first
cd blink-engine
bash ../setup.sh
```

### Issue: Build fails with "error: linking with `cc` failed"

**Solution on Linux:** Install required build tools
```bash
# Ubuntu/Debian
sudo apt-get install build-essential pkg-config libssl-dev

# CentOS/RHEL
sudo yum groupinstall "Development Tools"
sudo yum install pkg-config openssl-devel

# macOS (usually not needed, but if it happens)
xcode-select --install
```

### Issue: ".env file not found"

**Solution:** Create it from the example
```bash
cd blink-engine
cp .env.example .env
nano .env  # Edit with your values
```

### Issue: Tests fail during setup

**Solution:** This might not prevent running the engine. Check the error message:
```bash
cd blink-engine
cargo test --release
```

If tests fail but the engine builds, you can usually run it anyway. Contact support if concerned.

### Issue: "WebSocket connection refused"

**Possible causes:**
1. Invalid `WS_URL` in `.env`
2. Network connectivity issues
3. Polymarket service down

**Solution:** Verify .env is correct and test connectivity:
```bash
curl -i https://clob.polymarket.com/health
```

### Issue: PowerShell execution policy error on Windows

**Solution:**
```powershell
Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope Process
```

This only affects the current PowerShell session.

---

## Next Steps

After successful setup:

1. **Read the documentation**
   - Review `blink-engine/README.md` for architecture details
   - Check `blink-engine/CHANGELOG.md` for version info

2. **Explore market scanner**
   ```bash
   make run-market-scanner
   ```

3. **Paper trade**
   ```bash
   make run-paper
   ```

4. **Monitor logs**
   ```bash
   make logs-tail
   ```

5. **When ready for live trading**
   - Ensure all risk parameters are set
   - Test edge cases in paper mode
   - Start with small position sizes
   - Monitor continuously

---

## Support

For issues or questions:

- Check the [Common Issues](#common-issues) section above
- Review `blink-engine/README.md`
- Check log files in `blink-engine/logs/`
- Verify `.env` configuration

---

## Security Reminders

🔐 **Important:**
- Never commit `.env` to version control (it's in `.gitignore`)
- Never share your private key or API credentials
- Always test in read-only or paper mode first
- Keep the master `TRADING_ENABLED` flag off by default
- Review risk parameters carefully before live trading
- Monitor the engine during live operation

---

Last updated: 2025-02 | Blink Engine v0.2+
