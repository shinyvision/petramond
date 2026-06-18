#!/usr/bin/env bash
# Build the web bundle: compiles both wasm crates + runs wasm-bindgen.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> Build llamacraft lib (wasm32 release)"
cargo build --lib --target wasm32-unknown-unknown --release

echo "==> Build worker_wasm (wasm32 release)"
cargo build -p worker_wasm --target wasm32-unknown-unknown --release

echo "==> wasm-bindgen: llamacraft"
wasm-bindgen target/wasm32-unknown-unknown/release/llamacraft.wasm \
  --out-dir web --out-name llamacraft --target web

echo "==> wasm-bindgen: worker_wasm"
wasm-bindgen target/wasm32-unknown-unknown/release/worker_wasm.wasm \
  --out-dir web --out-name worker_wasm --target web

echo "==> Done. Run:  (cd web && PORT=8080 python3 dev_server.py)"
echo "    Open http://localhost:8080/"