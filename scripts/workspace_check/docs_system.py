from __future__ import annotations

import re
from pathlib import Path

from check_common.context import CheckContext


MAX_AGENTS_LINES = 80
MARKDOWN_LINK_RE = re.compile(r"!?\[[^\]]*\]\(([^)]+)\)")


def _read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def _ensure_contains(text: str, marker: str, *, path: Path) -> None:
    if marker not in text:
        raise SystemExit(f"check-workspace: {path.name} missing marker: {marker}")


def _normalize_markdown_target(raw_target: str) -> str:
    target = raw_target.strip()
    if not target:
        return ""
    if target.startswith("<") and target.endswith(">"):
        target = target[1:-1].strip()
    if " " in target:
        target = target.split(" ", 1)[0]
    return target


def _check_repo_local_markdown_links(repo_root: Path, path: Path) -> None:
    text = _read_text(path)
    for match in MARKDOWN_LINK_RE.finditer(text):
        target = _normalize_markdown_target(match.group(1))
        if not target or target.startswith("#"):
            continue
        if target.startswith(("/", "http://", "https://", "mailto:", "tel:")):
            continue
        target_path = target.split("#", 1)[0].split("?", 1)[0]
        if not target_path:
            continue
        resolved = (path.parent / target_path).resolve(strict=False)
        if not resolved.is_relative_to(repo_root):
            raise SystemExit(
                "check-workspace: markdown link escapes repository root: "
                f"{path.relative_to(repo_root)} -> {target}"
            )


def run_docs_system_checks(ctx: CheckContext) -> None:
    repo_root = ctx.repo_root

    readme = repo_root / "README.md"
    agents = repo_root / "AGENTS.md"
    docs_readme = repo_root / "docs" / "README.md"
    docs_system_map = repo_root / "docs" / "docs-system-map.md"
    docs_policy_index = repo_root / "docs" / "规范" / "README.md"
    docs_system = repo_root / "docs" / "规范" / "文档系统.md"
    architecture = repo_root / "ARCHITECTURE.md"

    required_files = [readme, agents, docs_readme, docs_system_map, docs_policy_index, docs_system, architecture]
    for path in required_files:
        if not path.is_file():
            raise SystemExit(f"check-workspace: missing required docs file: {path.relative_to(repo_root)}")

    readme_text = _read_text(readme)
    agents_text = _read_text(agents)
    docs_readme_text = _read_text(docs_readme)
    docs_system_map_text = _read_text(docs_system_map)
    docs_policy_index_text = _read_text(docs_policy_index)
    architecture_text = _read_text(architecture)

    agents_lines = len(agents_text.splitlines())
    if agents_lines > MAX_AGENTS_LINES:
        raise SystemExit(
            f"check-workspace: AGENTS.md is too long: {agents_lines} lines (limit {MAX_AGENTS_LINES})"
        )

    for marker in (
        "README.md",
        "docs/README.md",
        "docs/docs-system-map.md",
        "ARCHITECTURE.md",
        "docs/规范/README.md",
        "docs/规范/文档系统.md",
        "docs/crates/README.md",
    ):
        _ensure_contains(agents_text, marker, path=agents)

    for marker in ("AGENTS.md", "docs/README.md", "docs/docs-system-map.md"):
        _ensure_contains(readme_text, marker, path=readme)

    for marker in ("../AGENTS.md", "./docs-system-map.md", "./规范/文档系统.md"):
        _ensure_contains(docs_readme_text, marker, path=docs_readme)

    for marker in ("../README.md", "../AGENTS.md", "../ARCHITECTURE.md", "README.md"):
        _ensure_contains(docs_system_map_text, marker, path=docs_system_map)

    _ensure_contains(docs_policy_index_text, "./文档系统.md", path=docs_policy_index)
    _ensure_contains(architecture_text, "./AGENTS.md", path=architecture)

    docs_paths = [readme, agents, architecture]
    docs_paths.extend(sorted((repo_root / "docs").rglob("*.md")))
    for path in docs_paths:
        _check_repo_local_markdown_links(repo_root, path)
