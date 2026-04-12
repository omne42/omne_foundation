from __future__ import annotations

import os
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence


@dataclass(frozen=True)
class CheckContext:
    repo_root: Path
    python_executable: str


def normalize_repo_root(repo_root: str | Path) -> Path:
    return Path(repo_root).resolve()


def command_exists(command: str) -> bool:
    return shutil.which(command) is not None


def require_command(command: str, purpose: str) -> None:
    if command_exists(command):
        return
    raise SystemExit(
        f"pre-commit: missing required command for {purpose}: {command}"
    )


def resolve_cargo_command(ctx: CheckContext, purpose: str) -> str:
    cargo = shutil.which("cargo")
    if cargo:
        return cargo

    cargo_home = Path.home() / ".cargo" / "bin" / "cargo"
    if cargo_home.is_file() and os.access(cargo_home, os.X_OK):
        return str(cargo_home)

    raise SystemExit(
        "check-workspace: missing required command for "
        f"{purpose}: cargo (searched PATH and {cargo_home})"
    )


def with_cargo_command(
    ctx: CheckContext,
    args: Sequence[str | Path],
    *,
    purpose: str,
) -> list[str | Path]:
    if not args:
        return []
    if str(args[0]) != "cargo":
        return list(args)
    return [resolve_cargo_command(ctx, purpose), *args[1:]]


def _stringify_command(args: Sequence[str | Path]) -> list[str]:
    return [str(arg) for arg in args]


def _with_exdev_workaround(
    ctx: CheckContext,
    args: Sequence[str | Path],
    use_workaround: bool,
) -> list[str]:
    command = _stringify_command(args)
    if not use_workaround or os.name == "nt":
        return command

    workaround = ctx.repo_root / "scripts" / "with-rust-exdev-workaround.sh"
    if not workaround.is_file():
        return command

    return [str(workaround), *command]


def run_command(
    ctx: CheckContext,
    args: Sequence[str | Path],
    *,
    cwd: Path | None = None,
    env: Mapping[str, str] | None = None,
    use_workaround: bool = False,
    purpose: str | None = None,
) -> None:
    command = with_cargo_command(
        ctx,
        args,
        purpose=purpose or "workspace check",
    )
    command = _with_exdev_workaround(ctx, command, use_workaround)
    merged_env = os.environ.copy()
    if env is not None:
        merged_env.update(env)

    result = subprocess.run(
        command,
        cwd=str(cwd) if cwd is not None else None,
        env=merged_env,
        check=False,
    )
    if result.returncode != 0:
        raise SystemExit(result.returncode)


def capture_command(
    ctx: CheckContext,
    args: Sequence[str | Path],
    *,
    cwd: Path | None = None,
    allow_failure: bool = False,
    purpose: str | None = None,
) -> str:
    command = with_cargo_command(
        ctx,
        args,
        purpose=purpose or "workspace check",
    )
    result = subprocess.run(
        _stringify_command(command),
        cwd=str(cwd) if cwd is not None else None,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0 and not allow_failure:
        if result.stderr:
            sys.stderr.write(result.stderr)
        raise SystemExit(result.returncode)
    return result.stdout


def git_output(
    ctx: CheckContext,
    *args: str,
    allow_failure: bool = False,
) -> str:
    return capture_command(
        ctx,
        ["git", "-C", ctx.repo_root, *args],
        allow_failure=allow_failure,
    )


def git_show_text(ctx: CheckContext, spec: str) -> str | None:
    result = subprocess.run(
        ["git", "-C", str(ctx.repo_root), "show", spec],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return None
    return result.stdout


def ensure_no_symlink_components(path: Path) -> None:
    candidate = Path(path.anchor) if path.is_absolute() else Path()
    for part in path.parts:
        if part in ("", ".", path.anchor):
            continue
        candidate = candidate / part
        if candidate.is_symlink():
            raise SystemExit(f"pre-commit: path contains symlink component: {candidate}")


def clear_directory_contents(directory: Path) -> None:
    if not directory.exists():
        return

    for child in directory.iterdir():
        _remove_path(child)


def _remove_path(path: Path) -> None:
    if path.is_dir() and not path.is_symlink():
        _remove_directory(path)
        return

    path.unlink()


def _remove_directory(path: Path) -> None:
    for attempt in range(3):
        try:
            shutil.rmtree(path)
            return
        except OSError as err:
            # Some generated target trees briefly report ENOTEMPTY while directory entries settle.
            if path.exists() and err.errno in {39, 66} and attempt < 2:
                time.sleep(0.05)
                continue
            raise
