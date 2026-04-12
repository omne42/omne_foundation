from __future__ import annotations

import json

from check_common.context import CheckContext, capture_command


ALLOWED_INTERNAL_DEPS: dict[str, set[str]] = {
    "config-kit": set(),
    "error-kit": {"structured-text-kit"},
    "error-protocol": {"error-kit", "structured-text-kit", "structured-text-protocol"},
    "github-kit": {"http-kit"},
    "http-kit": set(),
    "i18n-kit": {"structured-text-kit"},
    "i18n-runtime-kit": {"i18n-kit", "structured-text-kit", "text-assets-kit"},
    "log-kit": {"structured-text-kit"},
    "mcp-jsonrpc": {"error-kit", "http-kit", "structured-text-kit"},
    "mcp-kit": {
        "config-kit",
        "error-kit",
        "http-kit",
        "mcp-jsonrpc",
        "structured-text-kit",
    },
    "notify-kit": {
        "github-kit",
        "http-kit",
        "log-kit",
        "secret-kit",
        "structured-text-kit",
    },
    "policy-meta": set(),
    "prompt-kit": {"text-assets-kit"},
    "secret-kit": {"error-kit", "structured-text-kit"},
    "structured-text-kit": set(),
    "structured-text-protocol": {"structured-text-kit"},
    "text-assets-kit": set(),
}


def _workspace_internal_deps(ctx: CheckContext) -> dict[str, set[str]]:
    metadata = capture_command(
        ctx,
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ctx.repo_root,
        purpose="cargo metadata for dependency-direction gate",
    )
    data = json.loads(metadata)
    workspace_prefix = f"{ctx.repo_root / 'crates'}"

    internal_deps: dict[str, set[str]] = {}
    for package in data["packages"]:
        manifest_path = package["manifest_path"]
        if not manifest_path.startswith(workspace_prefix):
            continue
        internal_deps[package["name"]] = {
            dependency["name"]
            for dependency in package["dependencies"]
            if dependency.get("path", "").startswith(workspace_prefix)
        }
    return internal_deps


def run_dependency_direction_checks(ctx: CheckContext) -> None:
    actual_internal_deps = _workspace_internal_deps(ctx)

    unknown_packages = sorted(set(actual_internal_deps) - set(ALLOWED_INTERNAL_DEPS))
    if unknown_packages:
        raise SystemExit(
            "check-workspace: dependency-direction missing packages in allowlist: "
            + ", ".join(unknown_packages)
        )

    stale_allowlist_entries = sorted(set(ALLOWED_INTERNAL_DEPS) - set(actual_internal_deps))
    if stale_allowlist_entries:
        raise SystemExit(
            "check-workspace: dependency-direction allowlist has unknown packages: "
            + ", ".join(stale_allowlist_entries)
        )

    violations: list[str] = []
    for package_name, actual_deps in sorted(actual_internal_deps.items()):
        allowed_deps = ALLOWED_INTERNAL_DEPS[package_name]
        unexpected_deps = sorted(actual_deps - allowed_deps)
        if unexpected_deps:
            violations.append(
                f"{package_name} -> unexpected internal deps: {', '.join(unexpected_deps)}"
            )
        stale_allowed_deps = sorted(allowed_deps - actual_deps)
        if stale_allowed_deps:
            violations.append(
                f"{package_name} -> stale allowlist deps: {', '.join(stale_allowed_deps)}"
            )

    if violations:
        raise SystemExit(
            "check-workspace: dependency-direction violations:\n- "
            + "\n- ".join(violations)
        )
