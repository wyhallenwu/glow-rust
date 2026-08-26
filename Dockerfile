FROM rust:bookworm AS builder
WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
RUN cargo build --release --locked

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /src/target/release/glow /usr/local/bin/glow
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/glow"]
