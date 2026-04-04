# ============================================================================
# Blink Engine — Multi-stage Docker Build
# Stage 1: builder  (rust:slim) — compile release binary
# Stage 2: runtime  (debian:bookworm-slim) — minimal attack surface
#
# Usage:
#   docker build -t blink-engine:latest .
#   docker run --env-file /etc/blink-engine.env --rm blink-engine:latest
# ============================================================================

# ── Stage 1: Build ───────────────────────────────────────────────────────────
FROM rust:1.80-slim AS builder

# System deps for openssl + k256 / sha3
RUN apt-get update -qq && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy manifests first to leverage layer caching for dependencies.
COPY Cargo.toml Cargo.lock ./
COPY crates/engine/Cargo.toml       crates/engine/Cargo.toml
COPY crates/tee-vault/Cargo.toml    crates/tee-vault/Cargo.toml
COPY crates/bpf-probes/Cargo.toml   crates/bpf-probes/Cargo.toml
COPY crates/market-scanner/Cargo.toml crates/market-scanner/Cargo.toml

# Stub source files so cargo can resolve the workspace graph without the full src.
RUN mkdir -p crates/engine/src \
             crates/tee-vault/src \
             crates/bpf-probes/src \
             crates/market-scanner/src && \
    echo "fn main() {}" > crates/engine/src/main.rs && \
    echo "" > crates/engine/src/lib.rs && \
    echo "" > crates/tee-vault/src/lib.rs && \
    echo "" > crates/bpf-probes/src/lib.rs && \
    echo "fn main() {}" > crates/market-scanner/src/main.rs

# Pre-build deps only (will be cached as long as Cargo.toml files don't change).
RUN cargo build --release 2>/dev/null || true

# Copy actual source code.
COPY crates/ crates/

# Touch main.rs to force cargo to recompile the binary.
RUN touch crates/engine/src/main.rs crates/engine/src/lib.rs

# Build the final release binary.
RUN cargo build -p engine --release

# ── Stage 2: Runtime ─────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update -qq && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
        tzdata \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root service user.
RUN useradd -r -m -d /opt/blink -s /bin/false blink

WORKDIR /opt/blink

# Copy the compiled binary from the builder stage.
COPY --from=builder /build/target/release/engine /opt/blink/engine

# Log and state directories.
RUN mkdir -p /opt/blink/logs/reports /opt/blink/logs/sessions && \
    chown -R blink:blink /opt/blink

USER blink

# Expose Agent RPC port (optional, only if AGENT_RPC_ENABLED=true).
EXPOSE 7878

# Default to paper/dry-run mode — operator must explicitly set LIVE_TRADING=true.
ENV LIVE_TRADING=false \
    PAPER_TRADING=true \
    TUI=false \
    LOG_LEVEL=info \
    PAPER_STATE_PATH=/opt/blink/logs/paper_portfolio_state.json \
    POSTRUN_REVIEW_DIR=/opt/blink/logs/reports \
    AUTO_POSTRUN_REVIEW=true

ENTRYPOINT ["/opt/blink/engine"]
