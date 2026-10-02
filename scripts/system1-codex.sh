#!/usr/bin/env bash
# Compatibility entry point for existing Cloud startup scripts.
set -euo pipefail
exec "$(dirname -- "${BASH_SOURCE[0]}")/decision-codex.sh" "$@"
