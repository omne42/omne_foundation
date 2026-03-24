#!/usr/bin/env bash
set -euo pipefail

crate_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ ! -f "$crate_root/Cargo.toml" ]]; then
  echo "pre-commit-check: no Cargo.toml found; skipping rust strict checks." >&2
  exit 0
fi

run_cargo_clippy() {
  local -a cargo_args=()
  local -a lint_args=()
  local parsing_lints=0
  local arg

  for arg in "$@"; do
    if [[ "$arg" == "--" && "$parsing_lints" -eq 0 ]]; then
      parsing_lints=1
      continue
    fi

    if [[ "$parsing_lints" -eq 0 ]]; then
      cargo_args+=("$arg")
    else
      lint_args+=("$arg")
    fi
  done

  # Match the workspace gate workaround so pre-commit keeps working when clippy-driver hits EXDEV.
  cargo clippy "${cargo_args[@]}" -- --emit=metadata=- "${lint_args[@]}" >/dev/null
}

echo "pre-commit-check: rust (clippy all-targets, deny warnings)" >&2
(
  cd "$crate_root"
  run_cargo_clippy --manifest-path "$crate_root/Cargo.toml" --all-targets -- -D warnings
)

echo "pre-commit-check: rust (strict production lints)" >&2
(
  cd "$crate_root"
  run_cargo_clippy \
    --manifest-path "$crate_root/Cargo.toml" \
    --all-features \
    --lib \
    --bins \
    --examples \
    -- \
    -D warnings \
    -W clippy::expect_used \
    -W clippy::let_underscore_must_use \
    -W clippy::map_clone \
    -W clippy::redundant_clone \
    -W clippy::unwrap_used
)
