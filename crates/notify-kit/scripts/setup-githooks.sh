#!/usr/bin/env bash
set -euo pipefail

crate_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
workspace_root="$(git -C "$crate_root" rev-parse --show-toplevel 2>/dev/null || true)"
if [[ -z "$workspace_root" ]]; then
  echo "setup-githooks: not a git repository; run: git init" >&2
  exit 1
fi

root_setup="$workspace_root/scripts/setup-githooks.sh"
if [[ ! -x "$root_setup" ]]; then
  echo "setup-githooks: missing workspace hook installer: $root_setup" >&2
  exit 1
fi

chmod +x "$crate_root/scripts/"*

exec "$root_setup"
