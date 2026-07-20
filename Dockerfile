FROM rust:1-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM scratch
COPY --from=builder /app/target/release/caddyrs /caddyrs
ENTRYPOINT ["/caddyrs"]
CMD ["--config", "/etc/caddyrs/caddy.rs.toml"]
