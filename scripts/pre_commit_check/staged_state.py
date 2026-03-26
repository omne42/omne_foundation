from __future__ import annotations

from dataclasses import dataclass
from pathlib import PurePosixPath

from .context import CheckContext, git_output


@dataclass(frozen=True)
class StagedState:
    paths: tuple[str, ...]
    changelog_paths: tuple[str, ...]
    deleted_paths: tuple[str, ...]
    has_changelog: bool
    non_changelog_count: int
    changed_crate_dirs: tuple[str, ...]
    crate_dirs_with_non_changelog_changes: tuple[str, ...]
    needs_policy_meta_assets: bool
    needs_mcp_assets: bool
    needs_notify_assets: bool


def is_changelog_path(path: str) -> bool:
    return path == "CHANGELOG.md" or (
        path.startswith("crates/") and path.endswith("/CHANGELOG.md")
    )


def needs_policy_meta_assets(path: str) -> bool:
    return path in {
        "crates/policy-meta/Cargo.toml",
        "crates/policy-meta/README.md",
        "crates/policy-meta/SPEC.md",
    } or path.startswith("crates/policy-meta/src/") or path.startswith(
        "crates/policy-meta/schema/"
    ) or path.startswith("crates/policy-meta/bindings/") or path.startswith(
        "crates/policy-meta/profiles/"
    )


def needs_mcp_assets(path: str) -> bool:
    return path in {
        "crates/mcp-kit/README.md",
        "crates/mcp-kit/CONTRIBUTING.md",
        "crates/mcp-kit/llms.txt",
        "crates/mcp-kit/examples/README.md",
    } or path.startswith("crates/mcp-kit/docs/") or path.startswith("crates/mcp-kit/scripts/")


def needs_notify_assets(path: str) -> bool:
    return path in {
        "crates/notify-kit/README.md",
        "crates/notify-kit/llms.txt",
    } or path.startswith("crates/notify-kit/docs/") or path.startswith(
        "crates/notify-kit/bots/"
    ) or path.startswith("crates/notify-kit/scripts/")


def crate_dir_for_path(path: str) -> str | None:
    parts = PurePosixPath(path).parts
    if len(parts) >= 2 and parts[0] == "crates":
        return parts[1]
    return None


def collect_staged_state(ctx: CheckContext) -> StagedState:
    output = git_output(
        ctx,
        "diff",
        "--cached",
        "--name-only",
        "--diff-filter=ACMRD",
        allow_failure=True,
    )
    paths = tuple(line.strip() for line in output.splitlines() if line.strip())
    deleted_output = git_output(
        ctx,
        "diff",
        "--cached",
        "--name-only",
        "--diff-filter=D",
        allow_failure=True,
    )
    deleted_paths = tuple(line.strip() for line in deleted_output.splitlines() if line.strip())
    changelog_paths = tuple(path for path in paths if is_changelog_path(path))
    non_changelog_count = sum(1 for path in paths if not is_changelog_path(path))
    changed_crate_dirs = tuple(
        sorted({crate_dir for path in paths if (crate_dir := crate_dir_for_path(path)) is not None})
    )
    crate_dirs_with_non_changelog_changes = tuple(
        sorted(
            {
                crate_dir
                for path in paths
                if not is_changelog_path(path)
                and (crate_dir := crate_dir_for_path(path)) is not None
            }
        )
    )
    return StagedState(
        paths=paths,
        changelog_paths=changelog_paths,
        deleted_paths=deleted_paths,
        has_changelog=bool(changelog_paths),
        non_changelog_count=non_changelog_count,
        changed_crate_dirs=changed_crate_dirs,
        crate_dirs_with_non_changelog_changes=crate_dirs_with_non_changelog_changes,
        needs_policy_meta_assets=any(needs_policy_meta_assets(path) for path in paths),
        needs_mcp_assets=any(needs_mcp_assets(path) for path in paths),
        needs_notify_assets=any(needs_notify_assets(path) for path in paths),
    )
