#!/usr/bin/env bash
# Build outside as a wasm32-wasip1 binary for the rootshell iOS terminal.
# Output: dist/outside.wasm
#
# Requirements:
#   - rustup with the wasm32-wasip1 target installed (auto-installed below)
#   - A rootshell host that provides the rootshell_socket_tls_connect_host
#     import. Without TLS, HTTPS endpoints (api.open-meteo.com etc.) are
#     unreachable from the WASM sandbox.
#
# Deployment: copy dist/outside.wasm into the rootshell Documents directory
# and invoke via `wasm outside …` from the in-app shell.

set -euo pipefail
cd "$(dirname "$0")"

rustup target add wasm32-wasip1 >/dev/null 2>&1 || true

cargo build --target wasm32-wasip1 --release

mkdir -p dist
cp target/wasm32-wasip1/release/outside.wasm dist/outside.wasm
ls -lh dist/outside.wasm
