# Build inside Debian: an Arch glibc-linked executable is not a Debian binary.
FROM rust:1.95.0-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends dpkg-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release --locked --no-default-features -p arc-deck-cli && \
    cargo test --locked -p arc-deck-core -p arc-deck-dsh -p arc-deck-tui -p arc-deck-cli --features arc-deck-cli/test-runtime
CMD ["bash", "packaging/debian/build.sh", "/src/target/release/arcdeck", "/out"]
