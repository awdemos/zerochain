#!/usr/bin/env bash
set -euo pipefail

workspace="${ZEROCHAIN_WORKSPACE:-/workspace}"
mkdir -p "$workspace"

if ! [ -d "$workspace/.jj" ]; then
    echo "==> Initializing jj repository..."
    jj init --git -R "$workspace" 2>/dev/null || true
    jj config set --repo user.name "zerochain" 2>/dev/null || true
    jj config set --repo user.email "zerochain@daemon" 2>/dev/null || true
fi

echo "==> Starting zerochaind on ${ZEROCHAIN_LISTEN:-0.0.0.0:8080}"
exec /usr/local/bin/zerochaind
