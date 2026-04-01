from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path

from check_common.context import CheckContext, run_command
from .dependency_direction import run_dependency_direction_checks
from .docs_system import run_docs_system_checks
from .mcp_kit_assets import run_mcp_kit_asset_checks
from .notify_kit_assets import run_notify_kit_asset_checks
from .policy_meta_assets import run_policy_meta_asset_checks
from .publish_contract import run_publish_contract_checks


def has_cargo_workspace(ctx: CheckContext) -> bool:
    return (ctx.repo_root / "Cargo.toml").is_file()


def workspace_member_packages(ctx: CheckContext) -> list[str]:
    metadata = subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ctx.repo_root,
        text=True,
    )
    data = json.loads(metadata)
    return [package["name"] for package in data["packages"]]


def run_local_checks(ctx: CheckContext) -> None:
    run_docs_system_checks(ctx)
    run_dependency_direction_checks(ctx)
    run_publish_contract_checks(ctx)
    fmt_command = ["cargo", "fmt"]
    for package in workspace_member_packages(ctx):
        fmt_command.extend(["-p", package])
    fmt_command.extend(["--", "--check"])
    run_command(
        ctx,
        fmt_command,
        cwd=ctx.repo_root,
    )
    run_command(
        ctx,
        ["cargo", "check", "--workspace", "--all-targets", "--all-features"],
        cwd=ctx.repo_root,
        use_workaround=True,
    )
    run_command(
        ctx,
        ["cargo", "test", "--workspace", "--all-features"],
        cwd=ctx.repo_root,
        use_workaround=True,
    )


def run_ci_checks(ctx: CheckContext) -> None:
    run_local_checks(ctx)
    run_command(
        ctx,
        ["cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"],
        cwd=ctx.repo_root,
        use_workaround=True,
    )
    run_asset_checks(ctx, "all")


def run_asset_checks(ctx: CheckContext, scope: str = "all") -> None:
    if scope == "all":
        run_policy_meta_asset_checks(ctx)
        run_mcp_kit_asset_checks(ctx)
        run_notify_kit_asset_checks(ctx)
        return
    if scope == "policy-meta":
        run_policy_meta_asset_checks(ctx)
        return
    if scope == "mcp-kit":
        run_mcp_kit_asset_checks(ctx)
        return
    if scope == "notify-kit":
        run_notify_kit_asset_checks(ctx)
        return
    raise SystemExit(f"check-workspace: unsupported asset scope: {scope}")


def run_secret_kit_target_check(ctx: CheckContext, target: str | None) -> None:
    if not target:
        raise SystemExit(
            "check-workspace: missing target triple for secret-kit-target mode"
        )
    run_command(
        ctx,
        ["cargo", "check", "-p", "secret-kit", "--target", target],
        cwd=ctx.repo_root,
        use_workaround=True,
    )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="check-workspace",
        description="Run workspace quality gates.",
    )
    parser.add_argument(
        "mode",
        nargs="?",
        default="local",
        choices=(
            "local",
            "ci",
            "docs-system",
            "dependency-direction",
            "publish-contract",
            "asset-checks",
            "secret-kit-target",
        ),
    )
    parser.add_argument("extra", nargs="?")
    parser.add_argument(
        "--repo-root",
        default=Path(__file__).resolve().parents[2],
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    ctx = CheckContext(
        repo_root=Path(args.repo_root).resolve(),
        python_executable="python3",
    )

    if args.mode == "local":
        run_local_checks(ctx)
        return 0
    if args.mode == "ci":
        run_ci_checks(ctx)
        return 0
    if args.mode == "docs-system":
        run_docs_system_checks(ctx)
        return 0
    if args.mode == "dependency-direction":
        run_dependency_direction_checks(ctx)
        return 0
    if args.mode == "publish-contract":
        run_publish_contract_checks(ctx)
        return 0
    if args.mode == "asset-checks":
        run_asset_checks(ctx, args.extra or "all")
        return 0
    if args.mode == "secret-kit-target":
        run_secret_kit_target_check(ctx, args.extra)
        return 0

    raise SystemExit("check-workspace: unsupported mode")
