#!/usr/bin/env bash
set -euo pipefail

crate_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

has_rust=0
has_python=0
has_node=0

if [[ -f "$crate_root/Cargo.toml" ]]; then
  has_rust=1
fi
if [[ -f "$crate_root/pyproject.toml" || -f "$crate_root/requirements-dev.txt" ]]; then
  has_python=1
fi
if find "$crate_root/bots" -maxdepth 2 -type f -name package.json -print -quit >/dev/null 2>&1; then
  has_node=1
fi

if [[ "$has_rust" -eq 0 && "$has_python" -eq 0 && "$has_node" -eq 0 ]]; then
  echo "gate: no supported project markers found (Cargo.toml / pyproject.toml / package.json); skipping." >&2
  exit 0
fi

if [[ "$has_rust" -eq 1 ]]; then
  echo "gate: rust (cargo fmt/check)" >&2
  (
    cd "$crate_root"
    cargo fmt --all --manifest-path "$crate_root/Cargo.toml" -- --check
    cargo check --manifest-path "$crate_root/Cargo.toml" --all-targets
  )
fi

if [[ "$has_python" -eq 1 ]]; then
  venv_python="$crate_root/.venv/bin/python"
  venv_ruff="$crate_root/.venv/bin/ruff"
  if [[ ! -x "$venv_python" || ! -x "$venv_ruff" ]]; then
    cat >&2 <<'EOF'
gate: python dev tools missing.

Run:
  ./scripts/bootstrap.sh
EOF
    exit 1
  fi

  echo "gate: python (ruff format/check + compileall)" >&2
  (
    cd "$crate_root"
    "$venv_ruff" format --check
    "$venv_ruff" check
    "$venv_python" -m compileall -q src
  )
fi

# Optional: run bot-local check scripts when present.
if command -v node >/dev/null 2>&1 && command -v npm >/dev/null 2>&1 && [[ -d "$crate_root/bots" ]]; then
  bot_packages=()
  while IFS= read -r f; do
    bot_packages+=("$f")
  done < <(find "$crate_root/bots" -maxdepth 2 -type f -name package.json -print 2>/dev/null)

  ran_bot_checks=0
  for pkg in "${bot_packages[@]}"; do
    if node -e 'const fs=require("fs");const p=process.argv[1];try{const j=JSON.parse(fs.readFileSync(p,"utf8"));process.exit(j?.scripts && typeof j.scripts.check==="string" ? 0 : 1)}catch{process.exit(2)}' "$pkg"; then
      pkg_dir="$(dirname "$pkg")"
      if [[ ! -d "$pkg_dir/node_modules" ]]; then
        cat >&2 <<EOF
gate: node dependencies missing ($pkg_dir/node_modules).

Run:
  ./scripts/bootstrap.sh
EOF
        exit 1
      fi

      echo "gate: node (npm run check in ${pkg_dir#$crate_root/})" >&2
      (
        cd "$crate_root"
        npm --prefix "$pkg_dir" run -s check
      )
      ran_bot_checks=1
    fi
  done

  if [[ "$ran_bot_checks" -eq 0 ]]; then
    echo "gate: node (no bot package check scripts found; skipped)" >&2
  fi
fi

# Optional: validate example bots syntax without installing deps.
if command -v node >/dev/null 2>&1 && [[ -d "$crate_root/bots" ]]; then
  bot_entrypoints=()
  while IFS= read -r f; do
    bot_entrypoints+=("$f")
  done < <(
    find "$crate_root/bots" -maxdepth 3 -type f \( -path "*/src/index.mjs" -o -path "*/src/index.js" \) -print 2>/dev/null
  )

  if [[ "${#bot_entrypoints[@]}" -gt 0 ]]; then
    echo "gate: node (bot syntax check)" >&2
    for f in "${bot_entrypoints[@]}"; do
      node --check "$f"
    done
  fi
fi
