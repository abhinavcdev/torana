FROM rust:1-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY torana-core ./torana-core
COPY torana ./torana
RUN cargo build --release -p torana

FROM scratch
COPY --from=builder /app/target/release/torana /torana
# Numeric UID/GID: scratch has no /etc/passwd for a named user, but the
# kernel only needs the numeric id. 65532 is the common "nonroot" convention
# (distroless, etc.) -- avoids running the proxy as root in the container.
USER 65532:65532
ENTRYPOINT ["/torana"]
CMD ["--config", "/etc/torana/torana.toml"]
