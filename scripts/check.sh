#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
pnpm --dir web install --frozen-lockfile
pnpm --dir web run format:check
pnpm --dir web run lint
pnpm --dir web run types:check
pnpm --dir web run typecheck
pnpm --dir web test
pnpm --dir web run build
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
pnpm --dir web run smoke
