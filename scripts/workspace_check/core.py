from __future__ import annotations

import argparse
import json
from pathlib import Path

from check_common.context import CheckContext, capture_command, run_command
from .dependency_direction import run_dependency_direction_checks
from .docs_system import run_docs_system_checks
from .mcp_kit_assets import run_mcp_kit_asset_checks
from .notify_kit_assets import run_notify_kit_asset_checks
from .policy_meta_assets import run_policy_meta_asset_checks
from .publish_contract import run_publish_contract_checks

HARDWARE_OPT_IN_FEATURE_PACKAGES = ("speech-whisper-kit",)


def has_cargo_workspace(ctx: CheckContext) -> bool:
    return (ctx.repo_root / "Cargo.toml").is_file()


def workspace_member_packages(ctx: CheckContext) -> list[str]:
    metadata = capture_command(
        ctx,
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ctx.repo_root,
        purpose="cargo metadata for workspace member discovery",
    )
    data = json.loads(metadata)
    return [package["name"] for package in data["packages"]]


def workspace_all_features_command(base: list[str]) -> list[str]:
    command = [*base, "--workspace"]
    for package in HARDWARE_OPT_IN_FEATURE_PACKAGES:
        command.extend(["--exclude", package])
    command.append("--all-features")
    return command


def run_hardware_opt_in_default_feature_checks(
    ctx: CheckContext,
    base: list[str],
    purpose: str,
) -> None:
    for package in HARDWARE_OPT_IN_FEATURE_PACKAGES:
        if "--" in base:
            separator_index = base.index("--")
            command = [
                *base[:separator_index],
                "-p",
                package,
                *base[separator_index:],
            ]
        else:
            command = [*base, "-p", package]
        run_command(
            ctx,
            command,
            cwd=ctx.repo_root,
            use_workaround=True,
            purpose=f"{purpose} default features ({package})",
        )


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
        purpose="cargo fmt workspace gate",
    )
    run_command(
        ctx,
        workspace_all_features_command(["cargo", "check", "--all-targets"]),
        cwd=ctx.repo_root,
        use_workaround=True,
        purpose="cargo check workspace gate",
    )
    run_hardware_opt_in_default_feature_checks(
        ctx,
        ["cargo", "check", "--all-targets"],
        "cargo check hardware opt-in gate",
    )
    run_command(
        ctx,
        workspace_all_features_command(["cargo", "test"]),
        cwd=ctx.repo_root,
        use_workaround=True,
        purpose="cargo test workspace gate",
    )
    run_hardware_opt_in_default_feature_checks(
        ctx,
        ["cargo", "test"],
        "cargo test hardware opt-in gate",
    )


def run_ci_checks(ctx: CheckContext) -> None:
    run_local_checks(ctx)
    run_command(
        ctx,
        [
            *workspace_all_features_command(["cargo", "clippy", "--all-targets"]),
            "--",
            "-D",
            "warnings",
        ],
        cwd=ctx.repo_root,
        use_workaround=True,
        purpose="cargo clippy workspace gate",
    )
    run_hardware_opt_in_default_feature_checks(
        ctx,
        ["cargo", "clippy", "--all-targets", "--", "-D", "warnings"],
        "cargo clippy hardware opt-in gate",
    )
    run_asset_checks(ctx, "all")


def run_review_root_checks(ctx: CheckContext) -> None:
    review_commands = (
        ["cargo", "check", "-p", "mcp-jsonrpc"],
        ["cargo", "check", "-p", "notify-kit"],
        ["cargo", "check", "-p", "policy-meta"],
        ["cargo", "check", "-p", "mcp-kit"],
        ["cargo", "test", "-p", "http-kit"],
        ["cargo", "test", "-p", "github-kit"],
    )
    for command in review_commands:
        run_command(
            ctx,
            command,
            cwd=ctx.repo_root,
            use_workaround=True,
            purpose=f"{command[0]} {' '.join(command[1:])} review-root gate",
        )


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
        purpose=f"cargo check secret-kit target gate ({target})",
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
            "review-root",
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
    if args.mode == "review-root":
        run_review_root_checks(ctx)
        return 0
    if args.mode == "secret-kit-target":
        run_secret_kit_target_check(ctx, args.extra)
        return 0

    raise SystemExit("check-workspace: unsupported mode")
