from __future__ import annotations

from .context import CheckContext, git_output


ALLOWED_BRANCH_PREFIXES = (
    "feat/",
    "fix/",
    "docs/",
    "refactor/",
    "perf/",
    "test/",
    "chore/",
    "build/",
    "ci/",
    "revert/",
)


def validate_branch_name(ctx: CheckContext) -> None:
    branch = git_output(ctx, "rev-parse", "--abbrev-ref", "HEAD", allow_failure=True).strip()
    if not branch or branch == "HEAD":
        return
    if branch in {"main", "master"}:
        return
    if branch.startswith(ALLOWED_BRANCH_PREFIXES):
        return

    allowed = ", ".join(ALLOWED_BRANCH_PREFIXES)
    raise SystemExit(
        "pre-commit: invalid branch name: "
        f"{branch}\n\n"
        "Branch must start with one of:\n"
        f"  {allowed}"
    )
