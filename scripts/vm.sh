#!/usr/bin/env bash
# Run a command inside the m3 development VM (OrbStack, Arch Linux) from macOS,
# in the repository directory, with a VM-local cargo target dir for speed.
#   scripts/vm.sh cargo test --workspace
set -euo pipefail
MACHINE="${M3_VM:-m3-dev}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
# The macOS→VM file share can lag a moment behind edits; let it settle.
sleep 1
exec orb -m "$MACHINE" bash -lc "cd '$REPO' && export CARGO_TARGET_DIR=\$HOME/m3-target CARGO_INCREMENTAL=0 && $(printf '%q ' "$@")"
