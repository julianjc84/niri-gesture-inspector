#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "=== Building niri-gesture-inspector (debug) ==="
cd "$SCRIPT_DIR"
cargo build

echo ""
echo "=== Stopping running instance ==="
killall niri-gesture-inspector 2>/dev/null && echo "Stopped." || echo "Not running."

echo ""
echo "=== Installing niri-gesture-inspector ==="
sudo cp "$SCRIPT_DIR/target/debug/niri-gesture-inspector" /usr/local/bin/niri-gesture-inspector

echo ""
echo "Done! Run 'niri-gesture-inspector' to open the live gesture scope."
