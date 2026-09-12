FROM rust:1-bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release

FROM debian:bookworm-slim

WORKDIR /app

COPY --from=builder /app/target/release/mercenary_board /app/mercenary_board

RUN mkdir -p /app/data

EXPOSE 3000

CMD ["/app/mercenary_board"]
