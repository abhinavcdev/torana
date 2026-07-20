FROM rust:1-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM scratch
COPY --from=builder /app/target/release/torana /torana
ENTRYPOINT ["/torana"]
CMD ["--config", "/etc/torana/torana.toml"]
