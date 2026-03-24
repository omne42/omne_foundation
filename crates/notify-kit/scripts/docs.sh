#!/usr/bin/env bash
set -euo pipefail

crate_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
workspace_root="$(git -C "$crate_root" rev-parse --show-toplevel 2>/dev/null || pwd)"
workaround_runner="$workspace_root/scripts/with-rust-exdev-workaround.sh"

run_with_rust_exdev_workaround() {
  if [[ -x "$workaround_runner" ]]; then
    "$workaround_runner" "$@"
  else
    "$@"
  fi
}

clear_directory_contents() {
  local dir="${1:-}"
  [[ -n "$dir" ]] || return 0
  [[ -d "$dir" ]] || return 0

  find "$dir" -mindepth 1 -depth -exec rm -f {} + 2>/dev/null || true
  find "$dir" -mindepth 1 -depth -type d -exec rmdir {} + 2>/dev/null || true
  find "$dir" -mindepth 1 -depth -delete 2>/dev/null || true
}

if ! command -v mdbook >/dev/null 2>&1; then
  cat >&2 <<'EOF'
docs: mdbook not found.

Install:
  cargo install mdbook --locked

Then run:
  ./scripts/docs.sh serve
EOF
  exit 1
fi

cmd="${1:-serve}"
case "$cmd" in
  serve)
    shift || true
    mdbook serve "$crate_root/docs" "$@"
    ;;
  build)
    shift || true
    mdbook build "$crate_root/docs" "$@"
    ;;
  test)
    shift || true
    docs_target_dir="$workspace_root/target/mdbook-test/notify-kit"
    mkdir -p "$docs_target_dir"
    clear_directory_contents "$docs_target_dir"
    (
      cd "$workspace_root"
      CARGO_TARGET_DIR="$docs_target_dir" run_with_rust_exdev_workaround cargo build --manifest-path "$crate_root/Cargo.toml"
    )
    run_with_rust_exdev_workaround mdbook test -L "$docs_target_dir/debug/deps" "$@" "$crate_root/docs"
    ;;
  *)
    cat >&2 <<'EOF'
Usage:
  ./scripts/docs.sh serve [mdbook args...]   # local preview with search
  ./scripts/docs.sh build [mdbook args...]   # build to target/mdbook/
  ./scripts/docs.sh test  [mdbook args...]   # compile Rust code snippets (requires cargo build)
EOF
    exit 2
    ;;
esac
