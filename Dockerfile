FROM rust:1.99.1-alpine@sha256:7e9e2d068a107a2adafd19930011178d14a78326053cd4c3a9f7ad3fd049557d AS builder

RUN apk add --no-cache musl-dev

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release --locked && rm -rf src

COPY src ./src
RUN touch src/main.rs && cargo build --release --locked

FROM alpine:latest@sha256:be2656061b536c1883c4034790e21d0a09c5aaaf51968b89368a3c0371ea2c9a

RUN adduser -D -u 10001 frametogether-server

COPY --from=builder /app/target/release/frametogether-server /usr/local/bin/

EXPOSE 47642

USER frametogether-server

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD wget -q -O- http://127.0.0.1:${PORT:-47642}/health || exit 1

CMD ["frametogether-server"]
