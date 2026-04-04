@echo off
REM ###############################################################################
REM                    Blink Engine Setup Script (Windows)
REM
REM Automated setup for the Blink Engine trading system on Windows
REM ###############################################################################

setlocal enabledelayedexpansion

cls

echo.
echo ╔════════════════════════════════════════════════════════╗
echo ║          Blink Engine Setup Script v1.0 (Windows)      ║
echo ╚════════════════════════════════════════════════════════╝
echo.

REM ###############################################################################
REM 1. Check if already in blink-engine directory
REM ###############################################################################

if not exist "Cargo.toml" (
    echo [INFO] Not in blink-engine directory. Attempting to navigate...

    if exist "blink-engine\" (
        cd blink-engine
        echo [OK] Navigated to blink-engine\
    ) else (
        echo [ERROR] Could not find blink-engine directory.
        echo [INFO] Please run this script from the root of the Blink repository
        pause
        exit /b 1
    )
)

REM ###############################################################################
REM 2. Check for Rust and Cargo
REM ###############################################################################

echo [INFO] Checking for Rust toolchain...

where rustc >nul 2>nul
if errorlevel 1 (
    echo [ERROR] Rust is not installed.
    echo [INFO] Please download and install from: https://rustup.rs/
    echo [INFO] Or use: choco install rust
    pause
    exit /b 1
)

for /f "tokens=2" %%i in ('rustc --version') do set RUST_VERSION=%%i
echo [OK] Found Rust: %RUST_VERSION%

where cargo >nul 2>nul
if errorlevel 1 (
    echo [ERROR] Cargo is not installed.
    pause
    exit /b 1
)

for /f "tokens=2" %%i in ('cargo --version') do set CARGO_VERSION=%%i
echo [OK] Found Cargo: %CARGO_VERSION%

REM ###############################################################################
REM 3. Check for .env file
REM ###############################################################################

echo.
echo [INFO] Checking environment configuration...

if exist ".env" (
    echo [OK] Found .env file
) else (
    if exist ".env.example" (
        echo [INFO] Creating .env from .env.example...
        copy .env.example .env >nul
        echo [OK] Created .env file
        echo [WARN] Please edit .env with your configuration:
        echo   - CLOB_HOST
        echo   - WS_URL
        echo   - RN1_WALLET
        echo   - MARKETS
        echo   - Live trading credentials (if using LIVE_TRADING=true)
    ) else (
        echo [ERROR] Neither .env nor .env.example found
        pause
        exit /b 1
    )
)

REM ###############################################################################
REM 4. Build the project
REM ###############################################################################

echo.
echo [INFO] Building Blink Engine...
echo [INFO] This may take several minutes on first build...
echo.

cargo build --release
if errorlevel 1 (
    echo [ERROR] Build failed. Check output above for errors.
    pause
    exit /b 1
)

echo [OK] Build completed successfully

REM ###############################################################################
REM 5. Optional: Run tests
REM ###############################################################################

echo.
set /p RUN_TESTS="Run tests? (y/n): "

if /i "%RUN_TESTS%"=="y" (
    echo [INFO] Running tests...
    cargo test --release
    if errorlevel 1 (
        echo [ERROR] Some tests failed. Check output above.
        pause
        exit /b 1
    )
    echo [OK] All tests passed
)

REM ###############################################################################
REM 6. Verify build artifacts
REM ###############################################################################

echo.
echo [INFO] Verifying build artifacts...

if exist "target\release\engine.exe" (
    echo [OK] Engine binary built successfully
) else (
    echo [ERROR] Engine binary not found after build
    pause
    exit /b 1
)

if exist "target\release\market-scanner.exe" (
    echo [OK] Market scanner binary built successfully
) else (
    echo [WARN] Market scanner binary not found
)

REM ###############################################################################
REM 7. Summary and next steps
REM ###############################################################################

echo.
echo [OK] Setup completed successfully!
echo.
echo Next steps:
echo.
echo 1. Edit .env with your configuration:
echo    notepad .env
echo.
echo 2. Discover markets (optional):
echo    cargo run -p market-scanner
echo.
echo 3. Run in read-only mode (recommended first):
echo    cargo run -p engine
echo.
echo 4. Run paper trading with dashboard:
echo    cmd /c "set PAPER_TRADING=true && set TUI=true && cargo run -p engine"
echo.
echo 5. For live trading (use with caution):
echo    cmd /c "set LIVE_TRADING=true && cargo run --release -p engine"
echo.
echo [WARN] Important security notes:
echo    * Never commit .env to version control
echo    * Always test in read-only or paper mode first
echo    * TRADING_ENABLED must be explicitly set to true
echo    * Verify all risk parameters before enabling live trading
echo.
echo Documentation: README.md
echo.

pause
