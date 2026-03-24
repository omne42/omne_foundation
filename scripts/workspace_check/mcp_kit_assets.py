from __future__ import annotations

import re
import sys
from pathlib import Path

from check_common.context import (
    CheckContext,
    ensure_no_symlink_components,
    require_command,
    run_command,
)


SUMMARY_ENTRY_RE = re.compile(r".*\[(.*)\]\((.*\.md)\).*")


def _resolve_doc_path(docs_dir: Path, relative_path: str) -> Path:
    if not relative_path:
        raise SystemExit("pre-commit: mcp-kit doc path must not be empty")
    if Path(relative_path).is_absolute():
        raise SystemExit(f"pre-commit: mcp-kit doc path must be relative: {relative_path}")

    candidate = Path(relative_path)
    if any(part == ".." for part in candidate.parts):
        raise SystemExit(
            "pre-commit: mcp-kit doc path must not contain '..' segments: "
            f"{relative_path}"
        )

    path = docs_dir / candidate
    if not path.is_file():
        raise SystemExit(
            "pre-commit: missing mcp-kit doc file referenced from SUMMARY.md: "
            f"{relative_path}"
        )

    ensure_no_symlink_components(path)
    resolved_path = path.resolve(strict=True)
    resolved_docs = docs_dir.resolve(strict=True)
    try:
        resolved_path.relative_to(resolved_docs)
    except ValueError as exc:
        raise SystemExit(
            "pre-commit: mcp-kit doc path escapes docs/: "
            f"{relative_path} (resolved to {resolved_path})"
        ) from exc
    return resolved_path


def _render_llms_bundle(crate_root: Path) -> str:
    docs_dir = crate_root / "docs"
    summary_path = docs_dir / "SUMMARY.md"

    ensure_no_symlink_components(docs_dir)
    if not summary_path.is_file():
        raise SystemExit("pre-commit: mcp-kit missing docs/SUMMARY.md")
    if summary_path.is_symlink():
        raise SystemExit("pre-commit: mcp-kit docs/SUMMARY.md must not be a symlink")

    parts = [
        "# mcp-kit docs (llms.txt)\n",
        "\n",
        "This file is generated from the Markdown docs in `docs/`, following `docs/SUMMARY.md`.\n",
        "It is meant to be pasted into LLM tooling (Cursor/Claude/ChatGPT) as a single context bundle.\n",
        "\n",
        "Regenerate: `./scripts/gen-llms-txt.sh`\n",
    ]

    for raw_line in summary_path.read_text(encoding="utf-8").splitlines():
        match = SUMMARY_ENTRY_RE.match(raw_line)
        if match is None:
            continue
        title, relative_path = match.group(1), match.group(2)
        if relative_path == "llms.md":
            continue

        doc_path = _resolve_doc_path(docs_dir, relative_path)
        parts.append("\n---\n\n")
        parts.append(f"# {title} ({relative_path})\n\n")
        parts.append(doc_path.read_text(encoding="utf-8"))

    return "".join(parts)


def _validate_llms_outputs(crate_root: Path, rendered: str) -> None:
    docs_output = crate_root / "docs" / "llms.txt"
    root_output = crate_root / "llms.txt"

    if not docs_output.is_file() or not root_output.is_file():
        raise SystemExit(
            "pre-commit: mcp-kit llms.txt outputs are missing; run ./scripts/gen-llms-txt.sh"
        )

    if docs_output.read_text(encoding="utf-8") != rendered:
        raise SystemExit(
            "pre-commit: crates/mcp-kit/docs/llms.txt is out of date; "
            "run ./scripts/gen-llms-txt.sh"
        )

    if root_output.read_text(encoding="utf-8") != rendered:
        raise SystemExit(
            "pre-commit: crates/mcp-kit/llms.txt is out of date; "
            "run ./scripts/gen-llms-txt.sh"
        )

    print("pre-commit: mcp-kit llms.txt is up to date", file=sys.stderr)


def run_mcp_kit_asset_checks(ctx: CheckContext) -> None:
    crate_root = ctx.repo_root / "crates" / "mcp-kit"
    docs_dir = crate_root / "docs"
    if not docs_dir.is_dir():
        return

    print("pre-commit: running mcp-kit asset checks", file=sys.stderr)
    require_command("mdbook", "mcp-kit docs")
    rendered = _render_llms_bundle(crate_root)
    _validate_llms_outputs(crate_root, rendered)
    run_command(ctx, ["mdbook", "build", docs_dir], cwd=ctx.repo_root)
