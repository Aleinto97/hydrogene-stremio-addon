# ===========================================
# Stage 1: Planner
# ===========================================
FROM lukemathwalker/cargo-chef:latest-rust-1.88-slim-bookworm AS planner
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ===========================================
# Stage 2: Cacher
# ===========================================
FROM lukemathwalker/cargo-chef:latest-rust-1.88-slim-bookworm AS cacher
WORKDIR /app
COPY --from=planner /app/recipe.json recipe.json

# Install dependencies for compilation
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    lld \
    g++ \
    && rm -rf /var/lib/apt/lists/*

# Build dependencies only (cached layer)
RUN cargo chef cook --release --recipe-path recipe.json

# ===========================================
# Stage 3: Builder
# ===========================================
FROM rust:1.88-slim-bookworm AS builder

# Install dependencies for compilation
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    lld \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .

# Copy pre-compiled dependencies
COPY --from=cacher /app/target target
COPY --from=cacher /usr/local/cargo /usr/local/cargo

# Build the actual application with fast linker
RUN RUSTFLAGS="-C link-arg=-fuse-ld=lld" cargo build --release --locked

# Strip binary to reduce size
RUN strip target/release/stremio-addon

# ===========================================
# Stage 4: Runner
# ===========================================
FROM debian:bookworm-slim AS runner

# Install runtime dependencies only
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libpq5 \
    curl \
    && rm -rf /var/lib/apt/lists/* \
    && apt-get clean

# Create non-root user
RUN groupadd -r appuser && useradd -r -g appuser appuser
WORKDIR /app
COPY --from=builder /app/target/release/stremio-addon /app/stremio-addon
RUN chown -R appuser:appuser /app
USER appuser
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/ || exit 1

CMD ["./stremio-addon"]