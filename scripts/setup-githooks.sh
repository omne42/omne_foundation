#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

git -C "$repo_root" config core.hooksPath githooks
chmod +x "$repo_root/githooks/"*

echo "Configured git hooks: core.hooksPath=githooks" >&2
echo "Hooks enabled: pre-commit, commit-msg" >&2
