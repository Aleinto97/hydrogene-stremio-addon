# ===========================================
# Stage 1: Builder
# Heavy container for compiling Rust code
# ===========================================
FROM rust:1.85-slim-bookworm AS builder

# Install required dependencies for compilation
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /app

# Copy Cargo files first for better caching
COPY Cargo.toml Cargo.lock ./

# Copy source code
COPY src ./src
COPY migrations ./migrations

# Build dependencies (cached layer)
RUN cargo build --release --locked 2>/dev/null || true

# Build the actual application
RUN cargo build --release --locked

# Strip binary to reduce size
RUN strip target/release/stremio-addon

# ===========================================
# Stage 2: Runner
# Minimal container for running the app
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

# Create non-root user for security
RUN groupadd -r appuser && useradd -r -g appuser appuser

# Set working directory
WORKDIR /app

# Copy binary from builder stage
COPY --from=builder /app/target/release/stremio-addon /app/stremio-addon

# Copy migrations
COPY --from=builder /app/migrations /app/migrations

# Change ownership to non-root user
RUN chown -R appuser:appuser /app

# Switch to non-root user
USER appuser

# Expose the port (Koyeb uses PORT env var)
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/ || exit 1

# Run the application
CMD ["./stremio-addon"]