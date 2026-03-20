FROM rust:1.94-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests
RUN cargo test --all-targets
RUN cargo build --release --bin gabysql --bin gabysql-server

FROM debian:bookworm-slim AS runtime
RUN useradd --create-home --home-dir /var/lib/gabysql --shell /usr/sbin/nologin gabysql
WORKDIR /app
COPY --from=builder /app/target/release/gabysql /usr/local/bin/gabysql
COPY --from=builder /app/target/release/gabysql-server /usr/local/bin/gabysql-server
RUN mkdir -p /data && chown -R gabysql:gabysql /data /var/lib/gabysql
USER gabysql
VOLUME ["/data"]
EXPOSE 8080
CMD ["gabysql-server", "-dir", "/data", "-addr", ":8080"]
