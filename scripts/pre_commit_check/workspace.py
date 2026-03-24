from __future__ import annotations

import sys

from workspace_check.core import has_cargo_workspace, run_local_checks

from .context import CheckContext


def run_local_workspace_checks(ctx: CheckContext) -> None:
    if not has_cargo_workspace(ctx):
        print("pre-commit: no Cargo workspace found; skipping Rust gates.", file=sys.stderr)
        return

    print(f"pre-commit: running Rust gates in {ctx.repo_root}", file=sys.stderr)
    run_local_checks(ctx)
