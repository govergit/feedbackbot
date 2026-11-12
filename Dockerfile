FROM rust:1.99-alpine AS builder
RUN apk add --no-cache build-base musl-dev perl make
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY migrations ./migrations
COPY src ./src
RUN cargo build --release
FROM alpine:3.20
RUN apk add --no-cache ca-certificates \
    && addgroup -S -g 10001 bot \
    && adduser -S -D -H -u 10001 -G bot bot \
    && mkdir -p /app/data \
    && chown -R 10001:10001 /app/data
WORKDIR /app
COPY --from=builder /app/target/release/tg-rust-support-bot /usr/local/bin/bot
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/bot"]
