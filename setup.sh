#!/bin/bash
set -e

MODEL_DIR="$HOME/1000Problems/models"
MODEL_FILE="$MODEL_DIR/Qwen2.5-7B-Instruct-Q5_K_M.gguf"

echo "Checking model at: $MODEL_FILE"

if [ ! -f "$MODEL_FILE" ]; then
    echo "Model not found. Please download Qwen2.5-7B-Instruct-Q5_K_M.gguf to $MODEL_DIR"
    exit 1
fi

echo "✓ Model found"
echo "Ready to run: cargo run -- smp.toml"
