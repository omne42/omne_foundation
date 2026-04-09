from __future__ import annotations

import sys
import tempfile
from pathlib import Path

from check_common.context import (
    CheckContext,
    command_exists,
    require_command,
    run_command,
)


def _run_llms_check(ctx: CheckContext, crate_root: Path) -> None:
    run_command(
        ctx,
        [ctx.python_executable, crate_root / "scripts" / "build-llms-txt.py", "--check"],
        cwd=crate_root,
    )


def _run_docs_test(ctx: CheckContext, crate_root: Path) -> None:
    docs_target_root = ctx.repo_root / "target" / "mdbook-test"
    docs_target_root.mkdir(parents=True, exist_ok=True)

    # Use a fresh target dir per run so repeated checks never race on shared `deps` cleanup.
    with tempfile.TemporaryDirectory(
        prefix="notify-kit-",
        dir=docs_target_root,
    ) as docs_target_dir_str:
        docs_target_dir = Path(docs_target_dir_str)
        # Some toolchains/runners assume the final deps tempdir ancestry already exists.
        (docs_target_dir / "debug" / "deps").mkdir(parents=True, exist_ok=True)

        env = {"CARGO_TARGET_DIR": str(docs_target_dir)}
        run_command(
            ctx,
            ["cargo", "build", "--manifest-path", crate_root / "Cargo.toml"],
            cwd=ctx.repo_root,
            env=env,
            use_workaround=True,
        )
        run_command(
            ctx,
            ["mdbook", "test", "-L", docs_target_dir / "debug" / "deps", crate_root / "docs"],
            cwd=ctx.repo_root,
            use_workaround=True,
        )


def _run_bot_syntax_checks(ctx: CheckContext, bots_root: Path) -> None:
    if not bots_root.is_dir():
        return

    if not command_exists("node"):
        print(
            "pre-commit: skipping notify-kit bot syntax checks because node is not installed",
            file=sys.stderr,
        )
        return

    entrypoints = sorted(bots_root.glob("*/src/index.js")) + sorted(
        bots_root.glob("*/src/index.mjs")
    )
    for entrypoint in entrypoints:
        run_command(ctx, ["node", "--check", entrypoint], cwd=ctx.repo_root)


def _run_shared_bot_tests(ctx: CheckContext, bots_root: Path) -> None:
    shared_root = bots_root / "_shared"
    if not shared_root.is_dir():
        return

    if not command_exists("node"):
        print(
            "pre-commit: skipping notify-kit shared bot tests because node is not installed",
            file=sys.stderr,
        )
        return

    test_files = sorted(shared_root.glob("*.test.mjs")) + sorted(
        shared_root.glob("*.test.js")
    )
    for test_file in test_files:
        run_command(ctx, ["node", "--test", test_file], cwd=ctx.repo_root)


def run_notify_kit_asset_checks(ctx: CheckContext) -> None:
    crate_root = ctx.repo_root / "crates" / "notify-kit"
    docs_dir = crate_root / "docs"
    if not docs_dir.is_dir():
        return

    print("pre-commit: running notify-kit asset checks", file=sys.stderr)
    require_command("mdbook", "notify-kit docs")
    _run_llms_check(ctx, crate_root)
    _run_docs_test(ctx, crate_root)
    _run_bot_syntax_checks(ctx, crate_root / "bots")
    _run_shared_bot_tests(ctx, crate_root / "bots")
