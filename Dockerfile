FROM rust:1.88-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release && rm -rf src

COPY src ./src
RUN touch src/main.rs && cargo build --release

FROM alpine:3.22

RUN apk add --no-cache ca-certificates

COPY --from=builder /app/target/release/frametogether-server /usr/local/bin/

EXPOSE 47642

USER nobody

CMD ["frametogether-server"]
