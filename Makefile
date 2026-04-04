.PHONY: help setup setup-check build build-release test test-verbose clean run run-paper run-paper-no-tui run-market-scanner run-live logs logs-tail lint fmt fmt-check check

# Colors for output
GREEN := \033[0;32m
YELLOW := \033[0;33m
BLUE := \033[0;34m
RED := \033[0;31m
NC := \033[0m # No Color

help: ## Show this help message
	@echo ""
	@echo "$(BLUE)Blink Engine - Makefile Targets$(NC)"
	@echo ""
	@echo "$(GREEN)Setup targets:$(NC)"
	@echo "  make setup              Run complete setup (Rust, dependencies, build)"
	@echo "  make setup-check        Check if setup is complete"
	@echo ""
	@echo "$(GREEN)Build targets:$(NC)"
	@echo "  make build              Build debug binary (fast)"
	@echo "  make build-release      Build optimized release binary"
	@echo "  make clean              Remove build artifacts"
	@echo ""
	@echo "$(GREEN)Testing:$(NC)"
	@echo "  make test               Run all tests"
	@echo "  make test-verbose       Run tests with verbose output"
	@echo ""
	@echo "$(GREEN)Running:$(NC)"
	@echo "  make run                Run engine in read-only mode"
	@echo "  make run-paper          Run engine in paper trading mode with TUI"
	@echo "  make run-market-scanner Run market scanner to discover markets"
	@echo "  make run-live           Run engine in live trading mode (CAUTION!)"
	@echo ""
	@echo "$(GREEN)Development:$(NC)"
	@echo "  make lint               Run clippy linter"
	@echo "  make fmt                Format code with rustfmt"
	@echo "  make fmt-check          Check code formatting without changes"
	@echo "  make check              Quick check without building"
	@echo ""
	@echo "$(GREEN)Logs:$(NC)"
	@echo "  make logs               Show available log files"
	@echo "  make logs-tail          Tail the latest session log (follow mode)"
	@echo ""

setup: ## Complete setup: check Rust, build project, create .env
	@echo "$(BLUE)Starting Blink Engine setup...$(NC)"
	@cd blink-engine && bash ../setup.sh || exit 1
	@echo ""

setup-check: ## Verify all prerequisites are installed
	@echo "$(BLUE)Checking prerequisites...$(NC)"
	@status=0; \
	command -v rustc > /dev/null 2>&1 && echo "$(GREEN)✓ Rust$(NC) $$(rustc --version)" || (echo "$(RED)✗ Rust not found$(NC)" && status=1); \
	command -v cargo > /dev/null 2>&1 && echo "$(GREEN)✓ Cargo$(NC) $$(cargo --version)" || (echo "$(RED)✗ Cargo not found$(NC)" && status=1); \
	command -v git > /dev/null 2>&1 && echo "$(GREEN)✓ Git$(NC) $$(git --version | head -1)" || (echo "$(RED)✗ Git not found$(NC)" && status=1); \
	if test -f blink-engine/.env; then \
		echo "$(GREEN)✓ .env configured$(NC)"; \
	else \
		echo "$(RED)✗ .env not found$(NC)"; \
		status=1; \
	fi; \
	if [ $$status -eq 0 ]; then \
		echo "$(GREEN)All prerequisites met!$(NC)"; \
	else \
		echo "$(RED)Some prerequisites are missing. Run 'make setup' to fix.$(NC)"; \
		exit 1; \
	fi

build: ## Build debug binary (fast compile, slower runtime)
	@cd blink-engine && cargo build
	@echo "$(GREEN)✓ Debug build complete$(NC)"

build-release: ## Build release binary (slow compile, optimized runtime)
	@cd blink-engine && cargo build --release
	@echo "$(GREEN)✓ Release build complete$(NC)"

clean: ## Remove build artifacts and clean up
	@cd blink-engine && cargo clean
	@echo "$(GREEN)✓ Cleaned$(NC)"

test: ## Run all tests
	@cd blink-engine && cargo test --release
	@echo "$(GREEN)✓ Tests passed$(NC)"

test-verbose: ## Run tests with verbose output
	@cd blink-engine && cargo test --release -- --nocapture --test-threads=1

check: ## Quick compilation check without building
	@cd blink-engine && cargo check
	@echo "$(GREEN)✓ Check passed$(NC)"

run: ## Run engine in read-only mode (watch RN1, no orders)
	@echo "$(BLUE)Starting Blink Engine in read-only mode...$(NC)"
	@cd blink-engine && cargo run -p engine
	@echo "$(GREEN)✓ Engine stopped$(NC)"

run-paper: ## Run paper trading with TUI dashboard
	@echo "$(BLUE)Starting Blink Engine in paper trading mode...$(NC)"
	@cd blink-engine && PAPER_TRADING=true TUI=true cargo run -p engine
	@echo "$(GREEN)✓ Engine stopped$(NC)"

run-paper-no-tui: ## Run paper trading without TUI
	@echo "$(BLUE)Starting Blink Engine in paper trading mode (no TUI)...$(NC)"
	@cd blink-engine && PAPER_TRADING=true cargo run -p engine
	@echo "$(GREEN)✓ Engine stopped$(NC)"

run-market-scanner: ## Discover Polymarket markets by volume
	@echo "$(BLUE)Running market scanner...$(NC)"
	@cd blink-engine && cargo run -p market-scanner

run-live: ## CAUTION: Run in live trading mode
	@echo "$(RED)╔════════════════════════════════════════════════════════╗$(NC)"
	@echo "$(RED)║                    LIVE TRADING MODE                   ║$(NC)"
	@echo "$(RED)║              Real money will be traded!                ║$(NC)"
	@echo "$(RED)╚════════════════════════════════════════════════════════╝$(NC)"
	@echo ""
	@echo "$(YELLOW)⚠  Make sure:$(NC)"
	@echo "  1. You have tested thoroughly in paper mode"
	@echo "  2. All risk parameters are verified in .env"
	@echo "  3. TRADING_ENABLED is set to true in your .env file"
	@echo "  4. LIVE_TRADING credentials are configured"
	@echo ""
	@trading_enabled=$$(grep '^TRADING_ENABLED=' blink-engine/.env 2>/dev/null | cut -d= -f2 || echo "false"); \
	if [ "$$trading_enabled" != "true" ]; then \
		echo "$(RED)Error: TRADING_ENABLED is not set to true in .env$(NC)"; \
		echo "$(YELLOW)Please enable it in blink-engine/.env before proceeding.$(NC)"; \
		exit 1; \
	fi; \
	read -p "Type 'YES' to proceed with live trading: " response; \
	if [ "$$response" = "YES" ]; then \
		cd blink-engine && LIVE_TRADING=true cargo run --release -p engine; \
	else \
		echo "$(GREEN)Cancelled$(NC)"; \
	fi

lint: ## Run clippy linter
	@cd blink-engine && cargo clippy --all-targets --all-features -- -D warnings
	@echo "$(GREEN)✓ No clippy warnings$(NC)"

fmt: ## Format code with rustfmt
	@cd blink-engine && cargo fmt
	@echo "$(GREEN)✓ Code formatted$(NC)"

fmt-check: ## Check code formatting without changing files
	@cd blink-engine && cargo fmt -- --check
	@echo "$(GREEN)✓ Code formatting is correct$(NC)"

logs: ## Show available log files
	@echo "$(BLUE)Log files:$(NC)"
	@test -d blink-engine/logs && ls -lh blink-engine/logs/ || echo "No logs directory found"
	@echo ""
	@echo "$(BLUE)Latest session log:$(NC)"
	@test -f blink-engine/logs/LATEST_SESSION_LOG.txt && cat blink-engine/logs/LATEST_SESSION_LOG.txt || echo "No session log found"

logs-tail: ## Follow the latest session log
	@echo "$(BLUE)Tailing latest session log...$(NC)"
	@tail -f "$$(cat blink-engine/logs/LATEST_SESSION_LOG.txt 2>/dev/null || echo "blink-engine/logs/engine.log")"

.DEFAULT_GOAL := help
