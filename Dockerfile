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

# Cache dependencies by building a dummy crate first
COPY Cargo.toml Cargo.lock ./
COPY crates/zerochain-cas/Cargo.toml crates/zerochain-cas/Cargo.toml
COPY crates/zerochain-fs/Cargo.toml crates/zerochain-fs/Cargo.toml
COPY crates/zerochain-llm/Cargo.toml crates/zerochain-llm/Cargo.toml
COPY crates/zerochain-core/Cargo.toml crates/zerochain-core/Cargo.toml
COPY crates/zerochain-engine/Cargo.toml crates/zerochain-engine/Cargo.toml
COPY crates/zerochain-broker/Cargo.toml crates/zerochain-broker/Cargo.toml
COPY crates/zerochain-daemon/Cargo.toml crates/zerochain-daemon/Cargo.toml
COPY crates/zerochain-server/Cargo.toml crates/zerochain-server/Cargo.toml
RUN mkdir -p crates/zerochain-cas/src crates/zerochain-fs/src crates/zerochain-llm/src \
    && mkdir -p crates/zerochain-core/src crates/zerochain-engine/src crates/zerochain-broker/src \
    && mkdir -p crates/zerochain-daemon/src crates/zerochain-server/src \
    && echo "pub fn main(){}" > crates/zerochain-daemon/src/main.rs \
    && echo "fn main(){}" > crates/zerochain-server/src/main.rs \
    && touch crates/zerochain-cas/src/lib.rs crates/zerochain-fs/src/lib.rs \
    && touch crates/zerochain-llm/src/lib.rs crates/zerochain-core/src/lib.rs \
    && touch crates/zerochain-engine/src/lib.rs crates/zerochain-broker/src/lib.rs \
    && touch crates/zerochain-server/src/lib.rs \
    && cargo build --release --bin zerochaind 2>/dev/null || true

# Now build with real source (dependency layer is cached)
COPY . .
RUN touch crates/*/src/*.rs && cargo build --release --bin zerochaind

# -- Runtime stage ---------------------------------------------------------
FROM cgr.dev/chainguard/wolfi-base:latest AS runtime

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
