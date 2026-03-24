#!/usr/bin/env python3
from __future__ import annotations

import re
import sys
from pathlib import Path


def extract_summary_paths(summary_md: str) -> list[str]:
    # GitBook/mdBook style: * [Title](path/to/file.md)
    paths = re.findall(r"\[[^\]]+\]\(([^)]+\.md)\)", summary_md)
    # Deduplicate while preserving order.
    seen: set[str] = set()
    out: list[str] = []
    for p in paths:
        p = p.strip()
        if not p or p in seen:
            continue
        seen.add(p)
        out.append(p)
    return out


def append_file(out: list[str], label: str, path: Path) -> None:
    if not path.is_file():
        print(f"build-llms-txt: warning: missing {label}", file=sys.stderr)
        return

    content = path.read_text(encoding="utf-8")
    if path.suffix == ".md":
        content = strip_mdbook_hidden_lines(content)

    out.append("\n---\n")
    out.append(f"## {label}\n\n")
    out.append(content)
    if not content.endswith("\n"):
        out.append("\n")


def strip_mdbook_hidden_lines(markdown: str) -> str:
    """
    Strip mdBook/Rustdoc hidden lines inside Rust code blocks.

    In mdBook, Rust code blocks can hide lines starting with `#` (but not `#[...]`),
    which are still part of the source file. For LLM bundles, these lines add noise.
    """

    out: list[str] = []
    in_rust_block = False

    for raw_line in markdown.splitlines(keepends=True):
        line = raw_line.rstrip("\n")

        if line.startswith("```"):
            tag = line.strip()
            if in_rust_block:
                in_rust_block = False
            else:
                in_rust_block = tag.startswith("```rust")
            out.append(raw_line)
            continue

        if in_rust_block:
            stripped = line.lstrip()
            if stripped.startswith("#") and not stripped.startswith("#["):
                continue

        out.append(raw_line)

    return "".join(out)


def main() -> int:
    repo_root = Path(__file__).resolve().parents[1]
    output_path = repo_root / "llms.txt"
    mode = "write"

    args = sys.argv[1:]
    if args:
        if args == ["--check"]:
            mode = "check"
        else:
            print("usage: ./scripts/build-llms-txt.sh [--check]", file=sys.stderr)
            return 2

    summary_path = repo_root / "docs" / "SUMMARY.md"
    if not summary_path.is_file():
        print("build-llms-txt: missing docs/SUMMARY.md", file=sys.stderr)
        return 1

    summary_paths = extract_summary_paths(summary_path.read_text(encoding="utf-8"))

    parts: list[str] = []
    parts.append("# notify-kit\n\n")
    parts.append("This file is an LLM-friendly bundle of the `notify-kit` documentation and examples.\n\n")
    parts.append("- Source of truth: `docs/` + `bots/`\n")
    parts.append("- Regenerate: `./scripts/build-llms-txt.sh`\n\n")

    for rel in summary_paths:
        rel = rel.lstrip("./")
        append_file(parts, f"docs/{rel}", repo_root / "docs" / rel)

    append_file(parts, "bots/README.md", repo_root / "bots" / "README.md")
    for readme in sorted((repo_root / "bots").glob("*/README.md")):
        append_file(parts, str(readme.relative_to(repo_root)), readme)

    rendered = "".join(parts)

    if mode == "check":
        if not output_path.is_file():
            print("build-llms-txt: missing llms.txt; run ./scripts/build-llms-txt.sh", file=sys.stderr)
            return 1

        current = output_path.read_text(encoding="utf-8")
        if current != rendered:
            print("build-llms-txt: llms.txt is out of date; run ./scripts/build-llms-txt.sh", file=sys.stderr)
            return 1

        print("build-llms-txt: llms.txt is up to date", file=sys.stderr)
        return 0

    output_path.write_text(rendered, encoding="utf-8")
    print(f"build-llms-txt: wrote {output_path}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
