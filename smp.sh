#!/bin/bash
set -e

cd ~/1000Problems/strategicMemoryPlanner

echo "Building smp..."
cargo build --release

echo "Starting smp daemon..."
./target/release/smp smp.toml "$@"
