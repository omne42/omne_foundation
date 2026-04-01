from __future__ import annotations

import os
import re
from pathlib import Path

from .context import CheckContext, git_show_text
from .repository_layout import RepositoryLayout
from .staged_state import StagedState


RELEASED_SECTION_RE = re.compile(r"^## \[[0-9]")


def crate_changelog_path(crate_dir: str) -> str:
    return f"crates/{crate_dir}/CHANGELOG.md"


def _existing_changelog(repo_root: Path, path: str) -> bool:
    return (repo_root / path).is_file()


def validate_allowed_changelog_paths(
    layout: RepositoryLayout,
    staged: StagedState,
) -> None:
    if layout.mode == "root":
        disallowed = [path for path in staged.changelog_paths if path != "CHANGELOG.md"]
        if not disallowed:
            return
        rendered = "\n".join(f"- {path}" for path in disallowed)
        raise SystemExit(
            "pre-commit: this repository uses a single root changelog.\n\n"
            "Do not update crate-local CHANGELOG.md files here.\n"
            "Use only:\n"
            "- CHANGELOG.md\n\n"
            "Disallowed staged changelog paths:\n"
            f"{rendered}"
        )

    if layout.mode != "crate":
        return

    disallowed = [
        path
        for path in staged.changelog_paths
        if path == "CHANGELOG.md" and path not in staged.deleted_paths
    ]
    deleted_crate_changelogs = {
        path
        for path in staged.deleted_paths
        if path.startswith("crates/") and path.endswith("/CHANGELOG.md")
        and path.rsplit("/", 1)[0] + "/Cargo.toml" in staged.deleted_paths
    }
    invalid = [
        path
        for path in staged.changelog_paths
        if path != "CHANGELOG.md"
        and path not in {crate_changelog_path(crate_dir) for crate_dir in layout.crate_dirs}
        and path not in deleted_crate_changelogs
    ]
    if not disallowed and not invalid:
        return

    details = [f"- {path}" for path in [*disallowed, *invalid]]
    raise SystemExit(
        "pre-commit: this repository keeps changelogs inside each crate.\n\n"
        "Root CHANGELOG.md is not allowed here.\n"
        "Only staged crate changelog paths matching crates/*/CHANGELOG.md are allowed.\n\n"
        "Disallowed staged changelog paths:\n"
        + "\n".join(details)
    )


def validate_changelog_update(
    ctx: CheckContext,
    layout: RepositoryLayout,
    staged: StagedState,
) -> None:
    if layout.mode == "none":
        return

    if layout.mode == "root":
        if "CHANGELOG.md" in staged.changelog_paths:
            return
        raise SystemExit(
            "pre-commit: a root-package repository must update CHANGELOG.md in the same commit.\n\n"
            "- Add an entry under [Unreleased] in:\n"
            "  - CHANGELOG.md\n"
            "- Stage that changelog before committing."
        )

    changed_dirs = [
        crate_dir
        for crate_dir in staged.crate_dirs_with_non_changelog_changes
        if crate_dir in layout.crate_dirs
    ]
    if not changed_dirs:
        return

    missing_files = [
        crate_dir
        for crate_dir in changed_dirs
        if not _existing_changelog(ctx.repo_root, crate_changelog_path(crate_dir))
        and crate_changelog_path(crate_dir) not in staged.changelog_paths
    ]
    if missing_files:
        rendered = "\n".join(
            f"- crates/{crate_dir}/CHANGELOG.md" for crate_dir in missing_files
        )
        raise SystemExit(
            "pre-commit: every crate-package must maintain its own changelog.\n\n"
            "Create the missing crate changelog file(s):\n"
            f"{rendered}"
        )

    missing_updates = [
        crate_dir
        for crate_dir in changed_dirs
        if crate_changelog_path(crate_dir) not in staged.changelog_paths
    ]
    if not missing_updates:
        return

    rendered = "\n".join(f"- crates/{crate_dir}/CHANGELOG.md" for crate_dir in missing_updates)
    raise SystemExit(
        "pre-commit: every changed crate-package must update its own changelog.\n\n"
        "Stage an [Unreleased] entry in:\n"
        f"{rendered}"
    )


def validate_not_changelog_only(staged: StagedState) -> None:
    if not staged.changelog_paths:
        return
    if staged.non_changelog_count > 0:
        return
    raise SystemExit(
        "pre-commit: refusing changelog-only commit; "
        "commit the actual change together with its changelog update."
    )


def _released_sections(text: str | None) -> str:
    if not text:
        return ""

    lines = text.splitlines()
    for index, line in enumerate(lines):
        if RELEASED_SECTION_RE.match(line):
            return "\n".join(lines[index:])
    return ""


def validate_released_sections_immutable(
    ctx: CheckContext,
    layout: RepositoryLayout,
    staged: StagedState,
) -> None:
    if os.environ.get("OMNE_ALLOW_CHANGELOG_RELEASE_EDIT") == "1":
        return

    if layout.mode == "root":
        relevant_paths = tuple(path for path in staged.changelog_paths if path == "CHANGELOG.md")
    elif layout.mode == "crate":
        allowed = {crate_changelog_path(crate_dir) for crate_dir in layout.crate_dirs}
        relevant_paths = tuple(path for path in staged.changelog_paths if path in allowed)
    else:
        relevant_paths = ()

    for path in relevant_paths:
        head_text = git_show_text(ctx, f"HEAD:{path}")
        index_text = git_show_text(ctx, f":{path}")
        if head_text is None or index_text is None:
            continue

        if _released_sections(head_text) == _released_sections(index_text):
            continue

        raise SystemExit(
            "pre-commit: refusing to modify released CHANGELOG sections.\n\n"
            "Only edit entries under [Unreleased]. Released version sections are immutable.\n\n"
            "If you are cutting a release and intentionally updating versioned sections, re-run with:\n"
            "  OMNE_ALLOW_CHANGELOG_RELEASE_EDIT=1 git commit ..."
        )
