# -- Build stage -----------------------------------------------------------
FROM ghcr.io/wolfi-dev/sdk:latest AS builder

RUN apk add --no-cache \
    bash \
    ca-certificates \
    curl \
    gcc \
    git \
    jq \
    musl-dev \
    && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly

ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /src
COPY . .

RUN cargo build --release --bin zerochaind

# -- Runtime stage ---------------------------------------------------------
FROM ghcr.io/wolfi-dev/static:latest

RUN apk add --no-cache \
    ca-certificates \
    git \
    jj \
    && adduser -D -u 1000 zerochain

COPY --from=builder /src/target/release/zerochaind /usr/local/bin/zerochaind
COPY container/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

ENV ZEROCHAIN_WORKSPACE=/workspace
ENV ZEROCHAIN_LISTEN=0.0.0.0:8080
ENV RUST_LOG=zerochaind=info

WORKDIR /workspace
VOLUME /workspace

USER zerochain
EXPOSE 8080

ENTRYPOINT ["/entrypoint.sh"]
