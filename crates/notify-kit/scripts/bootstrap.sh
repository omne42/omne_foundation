#!/usr/bin/env bash
set -euo pipefail

crate_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$crate_root"

bash ./scripts/setup-githooks.sh

if [[ -f "pyproject.toml" || -f "requirements-dev.txt" ]]; then
  if [[ ! -d ".venv" ]]; then
    python3 -m venv .venv
  fi
  ./.venv/bin/python -m pip install -U pip >/dev/null
  if [[ -f "requirements-dev.txt" ]]; then
    ./.venv/bin/pip install -r requirements-dev.txt >/dev/null
  fi
fi

while IFS= read -r package_json; do
  npm --prefix "$(dirname "$package_json")" install
done < <(find "$crate_root/bots" -maxdepth 2 -type f -name package.json -print 2>/dev/null)

echo "bootstrap: ok" >&2
