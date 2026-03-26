from __future__ import annotations

import sys

from check_common.context import CheckContext, run_command


def run_policy_meta_asset_checks(ctx: CheckContext) -> None:
    crate_root = ctx.repo_root / "crates" / "policy-meta"
    if not crate_root.is_dir():
        return

    print("pre-commit: running policy-meta asset checks", file=sys.stderr)
    run_command(
        ctx,
        ["cargo", "run", "-p", "policy-meta", "--bin", "export-artifacts", "--", "--check"],
        cwd=ctx.repo_root,
        use_workaround=True,
    )
