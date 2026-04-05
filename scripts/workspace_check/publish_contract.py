from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from pathlib import Path

from check_common.context import CheckContext, git_output


@dataclass(frozen=True)
class WorkspacePackage:
    name: str
    manifest_path: Path
    publish: list[str] | None
    dependency_names: tuple[str, ...]
    external_path_dependencies: tuple[tuple[str, Path], ...]

    @property
    def is_publish_false(self) -> bool:
        return self.publish == []

    @property
    def readme_path(self) -> Path:
        return self.manifest_path.parent / "README.md"


def _cargo_metadata(ctx: CheckContext) -> dict:
    metadata = subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ctx.repo_root,
        text=True,
    )
    return json.loads(metadata)


def _workspace_packages(ctx: CheckContext) -> dict[str, WorkspacePackage]:
    metadata = _cargo_metadata(ctx)
    repo_root = ctx.repo_root.resolve()
    packages: dict[str, WorkspacePackage] = {}
    for package in metadata["packages"]:
        manifest_path = Path(package["manifest_path"]).resolve()
        if repo_root not in manifest_path.parents:
            continue
        dependency_names = []
        external_path_dependencies = []
        for dependency in package["dependencies"]:
            dependency_path = dependency.get("path")
            if not dependency_path:
                continue
            dependency_path = Path(dependency_path).resolve()
            if dependency_path.is_relative_to(repo_root):
                dependency_names.append(dependency["name"])
                continue
            external_path_dependencies.append((dependency["name"], dependency_path))
        packages[package["name"]] = WorkspacePackage(
            name=package["name"],
            manifest_path=manifest_path,
            publish=package.get("publish"),
            dependency_names=tuple(dependency_names),
            external_path_dependencies=tuple(external_path_dependencies),
        )
    return packages


def _merge_base(ctx: CheckContext) -> str | None:
    merge_base = git_output(
        ctx,
        "merge-base",
        "HEAD",
        "origin/main",
        allow_failure=True,
    ).strip()
    return merge_base or None


def _changed_manifest_paths(ctx: CheckContext) -> set[Path]:
    changed_paths: set[Path] = set()
    merge_base = _merge_base(ctx)
    diff_ranges = []
    if merge_base:
        diff_ranges.append(["diff", "--name-only", f"{merge_base}...HEAD", "--", "crates/*/Cargo.toml"])
    diff_ranges.extend(
        [
            ["diff", "--name-only", "--cached", "--", "crates/*/Cargo.toml"],
            ["diff", "--name-only", "--", "crates/*/Cargo.toml"],
        ]
    )

    for args in diff_ranges:
        output = git_output(ctx, *args, allow_failure=True)
        for line in output.splitlines():
            if not line.strip():
                continue
            changed_paths.add((ctx.repo_root / line.strip()).resolve())
    return changed_paths


def _check_publish_false_readme_contract(
    ctx: CheckContext,
    packages: dict[str, WorkspacePackage],
) -> None:
    violations: list[str] = []
    for package in sorted(packages.values(), key=lambda item: item.name):
        if not package.is_publish_false or not package.readme_path.is_file():
            continue
        readme = package.readme_path.read_text(encoding="utf-8")
        if "crates.io" not in readme:
            continue
        rel_readme = package.readme_path.relative_to(ctx.repo_root)
        violations.append(
            f"{package.name}: {rel_readme} mentions crates.io but {package.manifest_path.relative_to(ctx.repo_root)} is publish = false"
        )

    if violations:
        details = "\n".join(f"- {violation}" for violation in violations)
        raise SystemExit(
            "check-workspace: publish contract regression detected.\n"
            "publish = false crates must not advertise crates.io installation in their README.\n"
            f"{details}"
        )


def _check_external_path_dependency_contract(
    ctx: CheckContext,
    packages: dict[str, WorkspacePackage],
) -> None:
    violations: list[str] = []
    for package in sorted(packages.values(), key=lambda item: item.name):
        if not package.external_path_dependencies:
            continue
        rel_manifest = package.manifest_path.relative_to(ctx.repo_root)
        for dependency_name, dependency_path in package.external_path_dependencies:
            violations.append(
                f"{package.name}: {rel_manifest} depends on `{dependency_name}` via external path "
                f"{dependency_path}; cross-repo foundation/runtime deps must use a canonical git "
                "source instead of escaping the workspace root"
            )

    if violations:
        details = "\n".join(f"- {violation}" for violation in violations)
        raise SystemExit(
            "check-workspace: publish contract regression detected.\n"
            "workspace packages must not depend on sibling/external path crates outside the "
            "repository root.\n"
            f"{details}"
        )


def run_publish_contract_checks(ctx: CheckContext) -> None:
    packages = _workspace_packages(ctx)
    _check_publish_false_readme_contract(ctx, packages)
    _check_external_path_dependency_contract(ctx, packages)
    changed_manifests = _changed_manifest_paths(ctx)
    if not changed_manifests:
        return

    manifest_to_package = {
        package.manifest_path: package for package in packages.values()
    }
    violations: list[str] = []
    for manifest_path in sorted(changed_manifests):
        package = manifest_to_package.get(manifest_path)
        if package is None or package.is_publish_false:
            continue

        unpublished_deps = [
            dependency_name
            for dependency_name in package.dependency_names
            if dependency_name in packages and packages[dependency_name].is_publish_false
        ]
        if unpublished_deps:
            deps = ", ".join(sorted(unpublished_deps))
            rel_manifest = manifest_path.relative_to(ctx.repo_root)
            violations.append(
                f"{package.name}: {rel_manifest} depends on publish=false workspace crate(s) "
                f"{deps}; mark this package publish = false or remove the unpublished dependency"
            )

    if violations:
        details = "\n".join(f"- {violation}" for violation in violations)
        raise SystemExit(
            "check-workspace: publish contract regression detected.\n"
            "A changed crate cannot keep implicit registry-publishable intent while depending on "
            "workspace-only crates declared with publish = false.\n"
            f"{details}"
        )
