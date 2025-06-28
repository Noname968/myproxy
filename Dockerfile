# syntax=docker/dockerfile:1

# ---- Build stage ----
    FROM rust:1.77 as builder

    # Install required tools
    RUN apt-get update && apt-get install -y pkg-config libssl-dev
    
    # Create a new empty project
    WORKDIR /app
    
    # Copy Cargo.toml and Cargo.lock first (for caching)
    COPY Cargo.toml Cargo.lock ./
    
    # Create a dummy src/main.rs to force a cargo fetch
    RUN mkdir src && echo "fn main() {}" > src/main.rs
    
    # Fetch dependencies
    RUN cargo fetch
    
    # Copy full source
    COPY . .
    
    # Build in release mode
    RUN cargo build --release
    
    # ---- Runtime stage ----
    FROM debian:bullseye-slim
    
    # Add a non-root user
    RUN useradd -m appuser
    
    # Only copy the built binary
    COPY --from=builder /app/target/release/myproxy /usr/local/bin/myproxy
    
    # Switch to non-root user
    USER appuser
    
    # Expose the port
    EXPOSE 3000
    
    # Run it
    ENTRYPOINT ["myproxy"]
    