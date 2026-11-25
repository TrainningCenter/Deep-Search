FROM rust:1.91-bookworm AS builder

WORKDIR /build
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release --bin deep-search

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg libgcc-s1 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 deepsearch \
    && useradd --uid 10001 --gid 10001 --create-home --shell /usr/sbin/nologin deepsearch \
    && mkdir -p /data /scan /tmp/deep-search \
    && chown -R 10001:10001 /data /tmp/deep-search

COPY --from=builder --chown=10001:10001 /build/target/release/deep-search /usr/local/bin/deep-search

USER 10001:10001
WORKDIR /data
VOLUME ["/data"]
EXPOSE 4765

# Container bridge binding is allowed only for the isolated Docker deployment.
# compose.yaml publishes this port on host loopback only.
ENV DEEP_SEARCH_CONTAINER_MODE=1
ENTRYPOINT ["deep-search", "serve", "/data/index.sqlite", "0.0.0.0:4765"]
