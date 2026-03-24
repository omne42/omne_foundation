from __future__ import annotations

import argparse
import sys
from pathlib import Path

from .assets import run_asset_checks
from .branch_name import validate_branch_name
from .changelog import (
    validate_allowed_changelog_paths,
    validate_changelog_update,
    validate_not_changelog_only,
    validate_released_sections_immutable,
)
from .context import CheckContext, normalize_repo_root
from .repository_layout import detect_repository_layout
from .staged_state import collect_staged_state
from .version_policy import run_version_policy_check
from .workspace import has_cargo_workspace, run_local_workspace_checks


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--repo-root",
        default=Path(__file__).resolve().parents[2],
    )
    parser.add_argument(
        "--layout",
        choices=("auto", "root", "crate"),
        default="auto",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    ctx = CheckContext(
        repo_root=normalize_repo_root(args.repo_root),
        python_executable=sys.executable or "python3",
    )

    validate_branch_name(ctx)
    staged = collect_staged_state(ctx)
    if not staged.paths:
        return 0

    layout = detect_repository_layout(ctx, expected_layout=args.layout)
    run_version_policy_check(ctx)
    validate_allowed_changelog_paths(layout, staged)
    validate_changelog_update(ctx, layout, staged)
    validate_not_changelog_only(staged)
    validate_released_sections_immutable(ctx, layout, staged)

    if not has_cargo_workspace(ctx):
        print("pre-commit: no Cargo workspace found; skipping Rust gates.", file=sys.stderr)
        return 0

    run_local_workspace_checks(ctx)
    run_asset_checks(ctx, staged)
    return 0
