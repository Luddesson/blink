# ============================================================================
# Blink HFT Engine — Production Docker Image (multi-stage)
# Target: linux/amd64
#
# Build:
#   docker build -t blink-engine:latest .
#
# Run (paper mode):
#   docker run --env-file .env blink-engine:latest
#
# Run (live mode — credentials required):
#   docker run --env-file /etc/blink-engine.env blink-engine:latest
# ============================================================================

# ── Stage 1: Build ──────────────────────────────────────────────────────────
FROM rust:1.83-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    clang \
    llvm \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache dependencies first (only re-runs when Cargo.toml/Cargo.lock change)
COPY Cargo.toml Cargo.lock ./
COPY crates/engine/Cargo.toml        crates/engine/Cargo.toml
COPY crates/tee-vault/Cargo.toml     crates/tee-vault/Cargo.toml
COPY crates/bpf-probes/Cargo.toml    crates/bpf-probes/Cargo.toml

# Dummy source files to warm up the dependency cache
RUN mkdir -p crates/engine/src crates/tee-vault/src crates/bpf-probes/src && \
    echo 'fn main(){}' > crates/engine/src/main.rs && \
    echo '' > crates/engine/src/lib.rs && \
    echo '' > crates/tee-vault/src/lib.rs && \
    echo '' > crates/bpf-probes/src/lib.rs

RUN cargo build --release -p engine 2>&1 | grep -v "^warning" || true
RUN rm -rf crates/*/src

# Full build
COPY crates crates
# Touch main files so cargo rebuilds them (timestamps were reset by the dummy build)
RUN touch crates/engine/src/lib.rs crates/engine/src/main.rs \
         crates/tee-vault/src/lib.rs crates/bpf-probes/src/lib.rs

RUN cargo build --release -p engine

# ── Stage 2: Runtime ────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create non-root service user
RUN groupadd -r blink && useradd -r -g blink -d /opt/blink -m blink

WORKDIR /opt/blink

# Copy engine binary
COPY --from=builder /build/target/release/engine /usr/local/bin/engine

# Create log directory owned by blink user
RUN mkdir -p logs/sessions logs/reports && chown -R blink:blink /opt/blink

USER blink

# Health: engine prints to stdout; no HTTP health endpoint in current version
# Use docker inspect / log monitoring instead.

# Default: paper mode (safe). Override LIVE_TRADING=true + credentials to go live.
ENV PAPER_TRADING=true
ENV TUI=false
ENV LOG_LEVEL=info

CMD ["/usr/local/bin/engine"]
