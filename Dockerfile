# ── Stage 1: Build the Rust binary ─────────────────────────────────────────────
FROM rust:1.97-slim-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests and source code
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations

# Build production binary
RUN cargo build --release

# ── Stage 2: Minimal runtime image with Python + ddgr ─────────────────────────
FROM python:3.11-slim-bookworm AS runner

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl-dev \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Install ddgr CLI for web searches
RUN pip install --no-cache-dir ddgr

# Install ccloud CLI for background monitoring
RUN curl -sS -L https://binaries.cockroachdb.com/ccloud/ccloud_linux-amd64_0.6.12.tar.gz | tar -xz \
    && mv ccloud /usr/local/bin/ccloud

# Copy the compiled binary from the builder stage
COPY --from=builder /app/target/release/arnheid /app/arnheid

# Expose port (if you run WhatsApp webhooks / metrics endpoints)
EXPOSE 8080

# Run the binary
CMD ["/app/arnheid"]
