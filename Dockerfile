FROM rust:1.99-alpine AS builder
RUN apk add --no-cache build-base musl-dev perl make
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY migrations ./migrations
COPY src ./src
RUN cargo build --release
FROM alpine:3.20
RUN apk add --no-cache ca-certificates
WORKDIR /app
COPY --from=builder /app/target/release/tg-rust-support-bot /usr/local/bin/bot
ENTRYPOINT ["/usr/local/bin/bot"]
