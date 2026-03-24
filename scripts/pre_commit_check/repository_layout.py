from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from .context import CheckContext


PACKAGE_SECTION_RE = re.compile(r"^\[package\]$")


@dataclass(frozen=True)
class RepositoryLayout:
    mode: str
    crate_dirs: tuple[str, ...]


def _manifest_has_package_section(manifest: Path) -> bool:
    if not manifest.is_file():
        return False

    for raw_line in manifest.read_text(encoding="utf-8").splitlines():
        line = raw_line.split("#", 1)[0].strip()
        if PACKAGE_SECTION_RE.match(line):
            return True
    return False


def detect_repository_layout(
    ctx: CheckContext,
    *,
    expected_layout: str = "auto",
) -> RepositoryLayout:
    root_manifest = ctx.repo_root / "Cargo.toml"
    has_root_package = _manifest_has_package_section(root_manifest)

    crate_dirs = tuple(
        sorted(
            manifest.parent.name
            for manifest in (ctx.repo_root / "crates").glob("*/Cargo.toml")
            if _manifest_has_package_section(manifest)
        )
    )

    if has_root_package and crate_dirs:
        rendered = "\n".join(f"- crates/{crate_dir}" for crate_dir in crate_dirs)
        raise SystemExit(
            "pre-commit: mixed changelog layouts are not allowed.\n\n"
            "Choose exactly one repository shape:\n"
            "- root-package repository: root Cargo.toml has [package], use only root CHANGELOG.md\n"
            "- crate-package directory: crates/*/Cargo.toml hold packages, each crate keeps its own CHANGELOG.md\n\n"
            "Detected root package plus crate packages:\n"
            f"{rendered}"
        )

    if has_root_package:
        layout = RepositoryLayout(mode="root", crate_dirs=())
    elif crate_dirs:
        layout = RepositoryLayout(mode="crate", crate_dirs=crate_dirs)
    else:
        layout = RepositoryLayout(mode="none", crate_dirs=())

    if expected_layout == "auto":
        return layout

    if expected_layout == layout.mode:
        return layout

    expected_rendered = {
        "root": "root-package repository",
        "crate": "crate-package directory",
    }[expected_layout]
    actual_rendered = {
        "root": "root-package repository",
        "crate": "crate-package directory",
        "none": "no package layout detected",
    }[layout.mode]
    raise SystemExit(
        "pre-commit: repository layout does not match hook expectation.\n\n"
        f"Expected: {expected_rendered}\n"
        f"Detected: {actual_rendered}"
    )
