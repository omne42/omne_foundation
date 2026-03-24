#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath


SEMVER_RE = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
TABLE_RE = re.compile(r"^\[(.+)\]$")
NAME_RE = re.compile(r'^name\s*=\s*"([^"]+)"$')
VERSION_RE = re.compile(r'^version\s*=\s*"([^"]+)"$')
VERSION_WORKSPACE_RE = re.compile(r"^version\.workspace\s*=\s*true$")
PATH_DEP_RE = re.compile(r'path\s*=\s*"((?:\.\./)+[^"]+)"')
BREAKING_COMMIT_RE = re.compile(
    r"^(feat|fix|docs|refactor|perf|test|chore|build|ci|revert)(\([a-z0-9._-]+\))?!: .+"
)


@dataclass(frozen=True)
class VersionTarget:
    path: str
    dir_name: str | None
    old_label: str | None
    new_label: str | None
    old_version: str | None
    new_version: str | None
    inherited_from_workspace: bool

    @property
    def display_label(self) -> str:
        return self.new_label or self.old_label or self.path

    @property
    def old_parts(self) -> tuple[int, int, int] | None:
        return parse_version(self.old_version)

    @property
    def new_parts(self) -> tuple[int, int, int] | None:
        return parse_version(self.new_version)

    @property
    def old_major(self) -> int | None:
        return None if self.old_parts is None else self.old_parts[0]

    @property
    def new_major(self) -> int | None:
        return None if self.new_parts is None else self.new_parts[0]

    @property
    def major_increased(self) -> bool:
        return (
            self.old_major is not None
            and self.new_major is not None
            and self.new_major > self.old_major
        )

    @property
    def introduces_nonzero_major(self) -> bool:
        return self.old_major is None and self.new_major not in (None, 0)

    @property
    def zero_major_unrestricted(self) -> bool:
        return self.old_major == 0 or self.new_major == 0


@dataclass(frozen=True)
class ApiDiff:
    package: str
    kind: str
    added: tuple[str, ...]
    removed: tuple[str, ...]


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--hook",
        choices=("pre-commit", "commit-msg"),
        default="pre-commit",
    )
    parser.add_argument("--commit-msg-file")
    parser.add_argument("--repo-root", default=None)
    return parser.parse_args(argv)


def run_command(
    args: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    allow_failure: bool = False,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        args,
        cwd=str(cwd) if cwd is not None else None,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0 and not allow_failure:
        if result.stderr:
            sys.stderr.write(result.stderr)
        raise SystemExit(result.returncode)
    return result


def git_text(repo_root: Path, *args: str, allow_failure: bool = False) -> str:
    result = run_command(
        ["git", "-C", str(repo_root), *args],
        allow_failure=allow_failure,
    )
    return result.stdout


def git_blob_text(repo_root: Path, spec: str) -> str | None:
    result = run_command(
        ["git", "-C", str(repo_root), "show", spec],
        allow_failure=True,
    )
    if result.returncode != 0:
        return None
    return result.stdout


def parse_version(version: str | None) -> tuple[int, int, int] | None:
    if version is None:
        return None
    match = SEMVER_RE.match(version)
    if not match:
        raise SystemExit(f"version-policy: unsupported non-semver version: {version}")
    return (int(match.group(1)), int(match.group(2)), int(match.group(3)))


def iter_section_lines(text: str | None, target_section: str):
    if text is None:
        return

    current_section = None
    for raw_line in text.splitlines():
        line = raw_line.split("#", 1)[0].strip()
        if not line:
            continue

        table_match = TABLE_RE.match(line)
        if table_match:
            current_section = table_match.group(1).strip()
            continue

        if current_section == target_section:
            yield line


def workspace_version(text: str | None) -> str | None:
    for line in iter_section_lines(text, "workspace.package"):
        version_match = VERSION_RE.match(line)
        if version_match:
            return version_match.group(1)
    return None


def package_name(text: str | None, fallback: str | None) -> str | None:
    if fallback is None and text is None:
        return None
    for line in iter_section_lines(text, "package"):
        name_match = NAME_RE.match(line)
        if name_match:
            return name_match.group(1)
    return fallback


def package_version(
    text: str | None,
    inherited_workspace_version: str | None,
) -> tuple[str | None, bool]:
    if text is None:
        return (None, False)
    for line in iter_section_lines(text, "package"):
        version_match = VERSION_RE.match(line)
        if version_match:
            return (version_match.group(1), False)
        if VERSION_WORKSPACE_RE.match(line):
            return (inherited_workspace_version, True)
    return (None, False)


def version_targets(repo_root: Path) -> list[VersionTarget]:
    index_root = git_blob_text(repo_root, ":Cargo.toml")
    head_root = git_blob_text(repo_root, "HEAD:Cargo.toml")
    index_workspace_version = workspace_version(index_root)
    head_workspace_version = workspace_version(head_root)

    targets: list[VersionTarget] = []

    for manifest in sorted((repo_root / "crates").glob("*/Cargo.toml")):
        rel_path = manifest.relative_to(repo_root).as_posix()
        dir_name = manifest.parent.name
        index_doc = git_blob_text(repo_root, f":{rel_path}")
        head_doc = git_blob_text(repo_root, f"HEAD:{rel_path}")

        if index_doc is None and head_doc is None:
            continue

        old_version, _ = package_version(head_doc, head_workspace_version)
        new_version, inherited = package_version(index_doc, index_workspace_version)

        targets.append(
            VersionTarget(
                path=rel_path,
                dir_name=dir_name,
                old_label=package_name(head_doc, dir_name),
                new_label=package_name(index_doc, dir_name),
                old_version=old_version,
                new_version=new_version,
                inherited_from_workspace=inherited,
            )
        )

    return targets


def active_index_targets(targets: list[VersionTarget]) -> list[VersionTarget]:
    return [target for target in targets if target.dir_name is not None and target.new_label is not None]


def require_single_versioning_mode(targets: list[VersionTarget]) -> str:
    active_targets = active_index_targets(targets)
    if not active_targets:
        return "crate"

    inherited = [target for target in active_targets if target.inherited_from_workspace]
    explicit = [target for target in active_targets if not target.inherited_from_workspace]

    if inherited and explicit:
        inherited_lines = "\n".join(
            f"- {target.display_label} [{target.path}]"
            for target in inherited
        )
        explicit_lines = "\n".join(
            f"- {target.display_label} [{target.path}]"
            for target in explicit
        )
        raise SystemExit(
            "version-policy: mixed versioning modes are not allowed.\n\n"
            "Choose exactly one mode for crates under crates/:\n"
            "- root-level mode: all crates use version.workspace = true\n"
            "- crate-level mode: all crates declare their own package.version\n\n"
            "Crates currently inheriting workspace version:\n"
            f"{inherited_lines}\n\n"
            "Crates currently using explicit package.version:\n"
            f"{explicit_lines}"
        )

    return "root" if inherited else "crate"


def major_change_targets(targets: list[VersionTarget]) -> list[VersionTarget]:
    changed: list[VersionTarget] = []
    for target in targets:
        if target.zero_major_unrestricted:
            continue
        if target.major_increased or target.introduces_nonzero_major:
            changed.append(target)
    return changed


def format_target(target: VersionTarget) -> str:
    inherited = " (inherits workspace version)" if target.inherited_from_workspace else ""
    return (
        f"- {target.display_label}: {target.old_version or '<none>'} -> "
        f"{target.new_version or '<none>'}{inherited} [{target.path}]"
    )


def require_major_bump_override(changed_targets: list[VersionTarget]) -> None:
    if not changed_targets:
        return
    if os.environ.get("OMNE_ALLOW_MAJOR_VERSION_BUMP") == "1":
        return
    details = "\n".join(format_target(target) for target in changed_targets)
    raise SystemExit(
        "version-policy: refusing major version change by default.\n\n"
        "The following version targets changed their major segment:\n"
        f"{details}\n\n"
        "Major version changes require an explicit override:\n"
        "  OMNE_ALLOW_MAJOR_VERSION_BUMP=1 git commit ...\n\n"
        "See docs/规范/版本与兼容.md for the policy."
    )


def require_breaking_commit_marker(
    msg_file: str | None,
    changed_targets: list[VersionTarget],
) -> None:
    if not changed_targets:
        return
    if not msg_file:
        raise SystemExit("version-policy: missing --commit-msg-file for commit-msg mode")
    lines = Path(msg_file).read_text(encoding="utf-8").splitlines()
    first_line = lines[0].strip() if lines else ""
    if BREAKING_COMMIT_RE.match(first_line):
        return
    details = "\n".join(format_target(target) for target in changed_targets)
    raise SystemExit(
        "version-policy: major version change requires an explicit breaking commit message.\n\n"
        "The following version targets changed their major segment:\n"
        f"{details}\n\n"
        "Use Conventional Commits with `!`, for example:\n"
        "  refactor(core)!: start 1.0 transition"
    )


def staged_paths(repo_root: Path) -> list[str]:
    output = git_text(
        repo_root,
        "diff",
        "--cached",
        "--name-only",
        "--diff-filter=ACMRD",
    )
    return [line.strip() for line in output.splitlines() if line.strip()]


def staged_crate_dirs(repo_root: Path) -> set[str]:
    dirs: set[str] = set()
    for path in staged_paths(repo_root):
        parts = PurePosixPath(path).parts
        if len(parts) >= 2 and parts[0] == "crates":
            dirs.add(parts[1])
    return dirs


def dependency_sibling_names(repo_root: Path) -> set[str]:
    names: set[str] = set()
    manifests = [repo_root / "Cargo.toml", *sorted((repo_root / "crates").glob("*/Cargo.toml"))]
    for manifest in manifests:
        if not manifest.exists():
            continue
        text = manifest.read_text(encoding="utf-8")
        for match in PATH_DEP_RE.finditer(text):
            rel = PurePosixPath(match.group(1))
            parts = list(rel.parts)
            while parts and parts[0] in (".", ".."):
                parts.pop(0)
            if parts:
                names.add(parts[0])
    return names


def link_external_path_dependencies(repo_root: Path, temp_root: Path) -> None:
    for sibling in dependency_sibling_names(repo_root):
        source = repo_root.parent / sibling
        if not source.exists():
            continue
        target = temp_root / sibling
        if target.exists():
            continue
        target.symlink_to(source, target_is_directory=source.is_dir())


def export_head_tree(repo_root: Path, head_dir: Path, archive_path: Path) -> bool:
    if not git_text(repo_root, "rev-parse", "--verify", "HEAD", allow_failure=True).strip():
        return False

    with archive_path.open("wb") as archive_file:
        result = subprocess.run(
            ["git", "-C", str(repo_root), "archive", "--format=tar", "HEAD"],
            stdout=archive_file,
            stderr=subprocess.PIPE,
            text=False,
            check=False,
        )
    if result.returncode != 0:
        if result.stderr:
            sys.stderr.write(result.stderr.decode("utf-8", errors="replace"))
        raise SystemExit(result.returncode)

    with tarfile.open(archive_path) as tar:
        tar.extractall(head_dir)
    return True


def export_index_tree(repo_root: Path, index_dir: Path) -> None:
    run_command(
        [
            "git",
            "-C",
            str(repo_root),
            "checkout-index",
            "--all",
            "--force",
            f"--prefix={index_dir.as_posix()}/",
        ]
    )


def normalize_json(value):
    if isinstance(value, dict):
        normalized = {}
        for key in sorted(value):
            if key in {"id", "crate_id", "span", "docs", "links"}:
                continue
            normalized[key] = normalize_json(value[key])
        return normalized
    if isinstance(value, list):
        return [normalize_json(item) for item in value]
    return value


def item_kind(item: dict) -> str:
    return next(iter(item["inner"]))


def item_meta(item: dict) -> dict:
    meta: dict[str, object] = {}
    if item.get("attrs"):
        meta["attrs"] = normalize_json(item["attrs"])
    if item.get("deprecation") is not None:
        meta["deprecation"] = normalize_json(item["deprecation"])
    return meta


def emit_summary(
    summaries: set[str],
    path: list[str],
    kind: str,
    payload,
    item: dict | None = None,
) -> None:
    data = {
        "path": "::".join(path),
        "kind": kind,
        "payload": normalize_json(payload),
    }
    if item is not None:
        data.update(item_meta(item))
    summaries.add(json.dumps(data, ensure_ascii=False, sort_keys=True))


def summarize_field(index: dict, field_id: int) -> dict:
    field = index[str(field_id)]
    return {
        "name": field.get("name"),
        "field": normalize_json(field["inner"]["struct_field"]),
        **item_meta(field),
    }


def summarize_composite_kind(index: dict, kind_data: dict) -> dict:
    if "plain" in kind_data:
        plain = kind_data["plain"]
        return {
            "plain": {
                "fields": [summarize_field(index, field_id) for field_id in plain.get("fields", [])],
                "has_stripped_fields": plain.get("has_stripped_fields", False),
            }
        }
    if "tuple" in kind_data:
        return {"tuple": [summarize_field(index, field_id) for field_id in kind_data["tuple"]]}
    if "unit" in kind_data:
        return {"unit": True}
    return normalize_json(kind_data)


def summarize_variant(index: dict, variant_id: int) -> dict:
    variant = index[str(variant_id)]
    variant_data = variant["inner"]["variant"]
    return {
        "name": variant.get("name"),
        "kind": summarize_composite_kind(index, variant_data["kind"]),
        "discriminant": normalize_json(variant_data.get("discriminant")),
        **item_meta(variant),
    }


def summarize_struct(index: dict, struct_data: dict) -> dict:
    return {
        "generics": normalize_json(struct_data.get("generics")),
        "kind": summarize_composite_kind(index, struct_data["kind"]),
    }


def summarize_enum(index: dict, enum_data: dict) -> dict:
    return {
        "generics": normalize_json(enum_data.get("generics")),
        "has_stripped_variants": enum_data.get("has_stripped_variants", False),
        "variants": [summarize_variant(index, variant_id) for variant_id in enum_data.get("variants", [])],
    }


def summarize_trait(trait_data: dict) -> dict:
    return {
        key: normalize_json(value)
        for key, value in trait_data.items()
        if key not in {"items", "implementations"}
    }


def export_associated_item(
    summaries: set[str],
    path: list[str],
    item: dict,
) -> None:
    kind = item_kind(item)
    emit_summary(summaries, path, f"associated_{kind}", item["inner"][kind], item)


def emit_trait_items(index: dict, path: list[str], item_ids: list[int], summaries: set[str]) -> None:
    for item_id in item_ids:
        item = index.get(str(item_id))
        if item is None or item.get("name") is None:
            continue
        export_associated_item(summaries, [*path, item["name"]], item)


def emit_impls(
    index: dict,
    path: list[str],
    impl_ids: list[int],
    summaries: set[str],
) -> None:
    for impl_id in impl_ids:
        impl = index.get(str(impl_id))
        if impl is None:
            continue
        impl_data = impl["inner"].get("impl")
        if impl_data is None or impl_data.get("is_synthetic"):
            continue

        if impl_data.get("trait") is not None:
            emit_summary(
                summaries,
                path,
                "trait_impl",
                {
                    "trait": normalize_json(impl_data["trait"]),
                    "generics": normalize_json(impl_data.get("generics")),
                    "is_unsafe": impl_data.get("is_unsafe", False),
                    "is_negative": impl_data.get("is_negative", False),
                },
                impl,
            )
            continue

        for item_id in impl_data.get("items", []):
            item = index.get(str(item_id))
            if item is None or item.get("visibility") != "public" or item.get("name") is None:
                continue
            export_associated_item(summaries, [*path, item["name"]], item)


def walk_module(
    index: dict,
    module_item: dict,
    path: list[str],
    summaries: set[str],
    seen: set[tuple[tuple[str, ...], int]],
    *,
    is_root: bool,
) -> None:
    if not is_root:
        emit_summary(summaries, path, "module", module_item["inner"]["module"], module_item)

    for child_id in module_item["inner"]["module"]["items"]:
        child = index.get(str(child_id))
        if child is None or child.get("visibility") != "public":
            continue

        if item_kind(child) == "use":
            use_data = child["inner"]["use"]
            alias = use_data["name"]
            target_id = use_data.get("id")
            alias_path = [*path, alias]
            if target_id is None or str(target_id) not in index:
                emit_summary(
                    summaries,
                    alias_path,
                    "reexport",
                    {
                        "source": use_data.get("source"),
                        "is_glob": use_data.get("is_glob", False),
                    },
                    child,
                )
                continue
            export_item(index, index[str(target_id)], alias_path, summaries, seen)
            continue

        name = child.get("name")
        if name is None:
            continue
        export_item(index, child, [*path, name], summaries, seen)


def export_item(
    index: dict,
    item: dict,
    path: list[str],
    summaries: set[str],
    seen: set[tuple[tuple[str, ...], int]],
) -> None:
    key = (tuple(path), int(item["id"]))
    if key in seen:
        return
    seen.add(key)

    kind = item_kind(item)
    inner = item["inner"][kind]

    if kind == "module":
        walk_module(index, item, path, summaries, seen, is_root=False)
        return

    if kind == "struct":
        emit_summary(summaries, path, kind, summarize_struct(index, inner), item)
        emit_impls(index, path, inner.get("impls", []), summaries)
        return

    if kind == "enum":
        emit_summary(summaries, path, kind, summarize_enum(index, inner), item)
        emit_impls(index, path, inner.get("impls", []), summaries)
        return

    if kind == "union":
        emit_summary(summaries, path, kind, inner, item)
        emit_impls(index, path, inner.get("impls", []), summaries)
        return

    if kind == "trait":
        emit_summary(summaries, path, kind, summarize_trait(inner), item)
        emit_trait_items(index, path, inner.get("items", []), summaries)
        return

    emit_summary(summaries, path, kind, inner, item)


def summarize_public_api(doc: dict) -> set[str]:
    index = doc["index"]
    root_item = index[str(doc["root"])]
    crate_name = root_item["name"]
    summaries: set[str] = set()
    seen: set[tuple[tuple[str, ...], int]] = set()
    walk_module(index, root_item, [crate_name], summaries, seen, is_root=True)
    return summaries


def load_rustdoc_json(json_path: Path) -> dict:
    with json_path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def build_rustdoc_json(tree_root: Path, package: str, target_dir: Path) -> dict:
    env = os.environ.copy()
    env["RUSTC_BOOTSTRAP"] = "1"
    env["CARGO_TARGET_DIR"] = str(target_dir)

    result = run_command(
        [
            "cargo",
            "rustdoc",
            "-q",
            "-p",
            package,
            "--all-features",
            "--lib",
            "--",
            "-Z",
            "unstable-options",
            "--output-format",
            "json",
        ],
        cwd=tree_root,
        env=env,
        allow_failure=True,
    )

    if result.returncode != 0:
        raise SystemExit(
            "version-policy: failed to build rustdoc JSON for public API diff.\n\n"
            f"package: {package}\n"
            f"tree: {tree_root}\n\n"
            f"{result.stderr}"
        )

    json_path = target_dir / "doc" / f"{package.replace('-', '_')}.json"
    if not json_path.exists():
        raise SystemExit(
            "version-policy: rustdoc JSON was not generated as expected.\n\n"
            f"package: {package}\n"
            f"expected: {json_path}"
        )
    return load_rustdoc_json(json_path)


def diff_public_api(head_doc: dict, index_doc: dict, package: str) -> ApiDiff:
    head_api = summarize_public_api(head_doc)
    index_api = summarize_public_api(index_doc)
    added = tuple(sorted(index_api - head_api))
    removed = tuple(sorted(head_api - index_api))

    if not added and not removed:
        kind = "none"
    elif removed:
        kind = "breaking"
    else:
        kind = "additive"

    return ApiDiff(package=package, kind=kind, added=added, removed=removed)


def render_api_entry(entry: str) -> str:
    data = json.loads(entry)
    rendered = f"{data['kind']} {data['path']}"
    payload = data.get("payload")
    if data["kind"] == "trait_impl" and isinstance(payload, dict):
        trait = payload.get("trait")
        if isinstance(trait, dict) and "path" in trait:
            rendered = f"{rendered} -> {trait['path']}"
    return rendered


def version_allows_api_change(target: VersionTarget, api_diff: ApiDiff) -> bool:
    old_parts = target.old_parts
    new_parts = target.new_parts
    if old_parts is None or new_parts is None:
        return True

    if target.zero_major_unrestricted:
        return True

    old_major, old_minor, _ = old_parts
    new_major, new_minor, _ = new_parts

    if api_diff.kind == "breaking":
        return new_major > old_major

    return new_major > old_major or (new_major == old_major and new_minor > old_minor)


def mismatch_reason(target: VersionTarget, api_diff: ApiDiff) -> str:
    old_parts = target.old_parts
    if old_parts is None:
        return "缺少可比较的旧版本信息"

    if api_diff.kind == "breaking":
        return "检测到 Rust public API breaking change，但版本没有提升大版本 a"

    return "检测到 Rust public API 新增/扩展，但版本没有提升小版本 b 或更高"


def package_targets_by_dir(targets: list[VersionTarget]) -> dict[str, VersionTarget]:
    return {
        target.dir_name: target
        for target in targets
        if target.dir_name is not None
    }


def require_public_api_version_alignment(
    repo_root: Path,
    targets: list[VersionTarget],
) -> None:
    staged_dirs = staged_crate_dirs(repo_root)
    if not staged_dirs:
        return

    target_map = package_targets_by_dir(targets)
    relevant = [target_map[dir_name] for dir_name in sorted(staged_dirs) if dir_name in target_map]
    relevant = [target for target in relevant if target.old_label and target.new_label]
    if not relevant:
        return

    (repo_root / ".tmp").mkdir(parents=True, exist_ok=True)
    temp_root_path = Path(
        tempfile.mkdtemp(prefix="version-policy-", dir=repo_root / ".tmp")
    )

    try:
        link_external_path_dependencies(repo_root, temp_root_path)

        head_dir = temp_root_path / "head"
        index_dir = temp_root_path / "index"
        head_dir.mkdir(parents=True, exist_ok=True)
        index_dir.mkdir(parents=True, exist_ok=True)

        if not export_head_tree(repo_root, head_dir, temp_root_path / "head.tar"):
            return
        export_index_tree(repo_root, index_dir)

        head_target_dir = temp_root_path / "target-head"
        index_target_dir = temp_root_path / "target-index"
        head_docs: dict[str, dict] = {}
        index_docs: dict[str, dict] = {}
        mismatches: list[str] = []

        for target in relevant:
            assert target.dir_name is not None
            if target.zero_major_unrestricted:
                continue

            if target.old_label != target.new_label:
                api_diff = ApiDiff(
                    package=target.display_label,
                    kind="breaking",
                    added=(f"package {target.new_label}",),
                    removed=(f"package {target.old_label}",),
                )
            else:
                package = target.new_label
                assert package is not None

                if package not in head_docs:
                    head_docs[package] = build_rustdoc_json(head_dir, package, head_target_dir)
                if package not in index_docs:
                    index_docs[package] = build_rustdoc_json(index_dir, package, index_target_dir)
                api_diff = diff_public_api(head_docs[package], index_docs[package], package)

            if api_diff.kind == "none":
                continue

            if version_allows_api_change(target, api_diff):
                continue

            details = [
                f"- {target.display_label}: {target.old_version or '<none>'} -> {target.new_version or '<none>'}",
                f"  - API change kind: {api_diff.kind}",
                f"  - Reason: {mismatch_reason(target, api_diff)}",
            ]

            if api_diff.removed:
                details.append("  - Removed or changed surface:")
                details.extend(
                    f"    - {render_api_entry(entry)}" for entry in api_diff.removed[:8]
                )
            if api_diff.added:
                details.append("  - Added surface:")
                details.extend(
                    f"    - {render_api_entry(entry)}" for entry in api_diff.added[:8]
                )

            mismatches.append("\n".join(details))

        if mismatches:
            raise SystemExit(
                "version-policy: staged Rust public API changes do not match the staged version bump.\n\n"
                "Each crate must version-match its own external surface.\n\n"
                + "\n\n".join(mismatches)
                + "\n\n"
                "This automated gate currently checks Rust public API from rustdoc JSON. "
                "CLI/config/protocol semantics still require engineering judgment.\n"
                "See docs/规范/版本与兼容.md for the policy."
            )
    finally:
        shutil.rmtree(temp_root_path, ignore_errors=True)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    repo_root = Path(
        args.repo_root or git_text(Path.cwd(), "rev-parse", "--show-toplevel").strip()
    )
    targets = version_targets(repo_root)
    require_single_versioning_mode(targets)
    changed_targets = major_change_targets(targets)

    require_major_bump_override(changed_targets)
    if args.hook == "pre-commit":
        require_public_api_version_alignment(repo_root, targets)
    if args.hook == "commit-msg":
        require_breaking_commit_marker(args.commit_msg_file, changed_targets)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
