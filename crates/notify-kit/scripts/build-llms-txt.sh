#!/usr/bin/env bash
set -euo pipefail

crate_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

if ! command -v python3 >/dev/null 2>&1; then
  cat >&2 <<'EOF'
build-llms-txt: python3 not found.

Install Python 3 and rerun:
  ./scripts/build-llms-txt.sh
EOF
  exit 1
fi

exec python3 "$crate_root/scripts/build-llms-txt.py" "$@"
