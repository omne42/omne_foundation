from __future__ import annotations

from version_policy.cli import main as run_version_policy_cli

from .context import CheckContext


def run_version_policy_check(ctx: CheckContext) -> None:
    run_version_policy_cli(
        [
            "--hook",
            "pre-commit",
            "--repo-root",
            str(ctx.repo_root),
        ]
    )
