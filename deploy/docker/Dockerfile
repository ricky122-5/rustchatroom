# syntax=docker/dockerfile:1.4

FROM rust:1.77-slim-bullseye AS builder
WORKDIR /app

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
COPY crates/p2p-node/Cargo.toml crates/p2p-node/Cargo.toml
COPY crates/p2p-cli/Cargo.toml crates/p2p-cli/Cargo.toml
RUN cargo fetch

# Build binary
COPY . .
RUN cargo build --release -p p2p-cli

FROM debian:bookworm-slim
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/p2p-cli /usr/local/bin/p2p-chat

ENV RUST_LOG=info
EXPOSE 9898

ENTRYPOINT ["p2p-chat", "run"]
CMD ["--listen", "/ip4/0.0.0.0/tcp/9000", "--metrics", "0.0.0.0:9898"]

