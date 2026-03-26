from __future__ import annotations

from check_common.context import CheckContext
from .staged_state import StagedState
from workspace_check.core import run_asset_checks as run_workspace_asset_checks


def run_asset_checks(ctx: CheckContext, staged: StagedState) -> None:
    if staged.needs_policy_meta_assets:
        run_workspace_asset_checks(ctx, "policy-meta")
    if staged.needs_mcp_assets:
        run_workspace_asset_checks(ctx, "mcp-kit")
    if staged.needs_notify_assets:
        run_workspace_asset_checks(ctx, "notify-kit")
