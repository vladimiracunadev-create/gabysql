FROM rust:1.97-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests
# Replicamos el orden de CI (.github/workflows/ci.yml) para que un
# `docker build` local atrape lo mismo: fmt → clippy → test → release.
# Sin esto, la única validación local era cargo test, y rustfmt/clippy
# se descubrían tarde en CI con un round-trip extra de push.
RUN rustup component add rustfmt clippy
RUN cargo fmt --check
RUN cargo clippy --all-targets -- -D warnings
RUN cargo test --all-targets
RUN cargo build --release --bin gabysql --bin gabysql-server

FROM debian:bookworm-slim AS runtime

# Apply security updates available in Debian's repos at build time. This
# does NOT close CVEs that Debian marks `(won't fix)` (those have no patch
# upstream), but it does pull in any CVE that already has a published fix
# — keeping the image as close as possible to the latest patched state.
# DEBIAN_FRONTEND=noninteractive avoids prompts; --no-install-recommends
# keeps the image small.
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get upgrade -y --no-install-recommends \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --home-dir /var/lib/gabysql --shell /usr/sbin/nologin gabysql
WORKDIR /app
COPY --from=builder /app/target/release/gabysql /usr/local/bin/gabysql
COPY --from=builder /app/target/release/gabysql-server /usr/local/bin/gabysql-server
RUN mkdir -p /data && chown -R gabysql:gabysql /data /var/lib/gabysql
USER gabysql
VOLUME ["/data"]
EXPOSE 8080
CMD ["gabysql-server", "-dir", "/data", "-addr", ":8080"]
