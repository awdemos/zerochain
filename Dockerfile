# -- Build stage -----------------------------------------------------------
FROM ghcr.io/wolfi-dev/sdk:latest AS builder

RUN apk add --no-cache \
    bash \
    ca-certificates \
    curl \
    gcc \
    git \
    glibc-dev \
    openssl-dev \
    && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly

ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /src
COPY . .

RUN cargo build --release --bin zerochaind

# -- Runtime stage ---------------------------------------------------------
FROM ghcr.io/wolfi-dev/sdk:latest AS runtime

RUN apk add --no-cache \
    bash \
    btrfs-progs \
    ca-certificates \
    git \
    && adduser -D -u 1000 zerochain

COPY --from=builder /src/target/release/zerochaind /usr/local/bin/zerochaind
COPY container/entrypoint.sh /entrypoint.sh
RUN chmod 0755 /entrypoint.sh /usr/local/bin/zerochaind

ENV ZEROCHAIN_WORKSPACE=/workspace
ENV ZEROCHAIN_LISTEN=0.0.0.0:8080
ENV RUST_LOG=zerochaind=info

WORKDIR /workspace
RUN chown zerochain:zerochain /workspace
VOLUME /workspace

USER zerochain
EXPOSE 8080

ENTRYPOINT ["/bin/bash", "/entrypoint.sh"]
