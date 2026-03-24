#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

ensure_linux_exdev_rename_preload() {
  case "$(uname -s)" in
    Linux) ;;
    *) return 0 ;;
  esac

  local src="$repo_root/scripts/native/rename_exdev_fix.c"
  local out_dir="$repo_root/.tmp/rust-exdev-fix"
  local so="$out_dir/librename_exdev_fix.so"
  local cc_bin

  [[ -f "$src" ]] || return 0

  cc_bin="$(command -v cc || command -v gcc || true)"
  [[ -n "$cc_bin" ]] || return 0

  mkdir -p "$out_dir"

  if [[ ! -s "$so" || "$src" -nt "$so" ]]; then
    "$cc_bin" -shared -fPIC -O2 -o "$so" "$src" -ldl
  fi

  case ":${LD_PRELOAD:-}:" in
    *":$so:"*) ;;
    *)
      if [[ -n "${LD_PRELOAD:-}" ]]; then
        export LD_PRELOAD="$so:$LD_PRELOAD"
      else
        export LD_PRELOAD="$so"
      fi
      ;;
  esac
}

ensure_linux_exdev_rename_preload

exec "$@"
