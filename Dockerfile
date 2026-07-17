# syntax=docker/dockerfile:1.7

FROM rust:1.88-bookworm AS rust-builder
WORKDIR /workspace
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --locked --release -p kcoin-node -p kcoin-cli

FROM debian:bookworm-slim AS node
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system kcoin \
    && useradd --system --gid kcoin --home-dir /data --shell /usr/sbin/nologin kcoin \
    && mkdir -p /data \
    && chown kcoin:kcoin /data
COPY --from=rust-builder /workspace/target/release/kcoin-node /usr/local/bin/kcoin-node
COPY --from=rust-builder /workspace/target/release/kcoin /usr/local/bin/kcoin
USER kcoin
VOLUME ["/data"]
EXPOSE 4100/tcp 5100/udp
ENTRYPOINT ["kcoin-node"]
CMD ["--role", "standalone", "--api-addr", "0.0.0.0:4100", "--p2p-port", "5100", "--db-path", "/data/node.db", "--demo"]

FROM node:22-alpine AS web-builder
WORKDIR /workspace/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web ./
RUN VITE_DEMO_MODE=never npm run build

FROM nginx:1.27-alpine AS web
COPY docker/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=web-builder /workspace/web/dist /usr/share/nginx/html
EXPOSE 80/tcp
HEALTHCHECK --interval=5s --timeout=2s --start-period=5s --retries=12 \
    CMD wget --quiet --spider http://127.0.0.1/ || exit 1
