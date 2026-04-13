#!/bin/bash
set -e

cd "$(dirname "$0")"

echo "Building smp (release)..."
cargo build --release

if [ ! -f ./target/release/smp ]; then
    echo "Build failed: binary not found"
    exit 1
fi

mkdir -p ~/.local/bin
ln -sf "$(pwd)/target/release/smp" ~/.local/bin/smp

echo "✓ smp ready — run with: smp"
