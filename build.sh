#!/bin/bash
set -e

# --- Configuration ---
LINUX_TARGET="x86_64-unknown-linux-gnu" # For Linux and WSL
MACOS_ARM_TARGET="aarch64-apple-darwin" # For Apple Silicon Macs
WINDOWS_TARGET="x86_64-pc-windows-gnu" # For Windows
BINARY_NAME="aws-inventory-sdk"

# --- Build Process ---
echo "Building for Linux (x86_64)..."
cargo build --release --target $LINUX_TARGET

echo "Building for macOS (Apple Silicon)..."
cargo build --release --target $MACOS_ARM_TARGET

echo "Building for Windows (x86_64) with size optimizations..."
cargo build --release --target $WINDOWS_TARGET

# --- Packaging ---
echo "Packaging binaries into 'dist' directory..."
DIST_DIR="dist"
rm -rf "$DIST_DIR" # Clean up previous builds
mkdir -p "$DIST_DIR"

cp "target/$LINUX_TARGET/release/$BINARY_NAME" "$DIST_DIR/${BINARY_NAME}-linux-amd64"
cp "target/$MACOS_ARM_TARGET/release/$BINARY_NAME" "$DIST_DIR/${BINARY_NAME}-macos-arm64"
cp "target/$WINDOWS_TARGET/release/${BINARY_NAME}.exe" "$DIST_DIR/${BINARY_NAME}-windows-amd64.exe"

echo "--- Build Complete! ---"
ls -l "$DIST_DIR"
