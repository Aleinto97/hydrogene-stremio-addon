# ===========================================
# Stage 1: Builder (Rust official image)
# ===========================================
FROM rust:1.88-slim-bookworm AS builder

# Install system dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    lld \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Download and compile dependencies first
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && echo "pub fn dummy() {}" > src/lib.rs
RUN cargo build --release && rm -rf src

# Copy actual source and rebuild
COPY src ./src
RUN cargo build --release

# ===========================================
# Stage 2: Runner
# ===========================================
FROM debian:bookworm-slim AS runner

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libpq5 \
    && rm -rf /var/lib/apt/lists/*

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
