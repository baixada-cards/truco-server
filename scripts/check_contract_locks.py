"""Verify all Rust dependencies match the immutable server contract lock."""

from __future__ import annotations

import json
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    lock = json.loads((ROOT / "contracts.lock.json").read_text(encoding="utf-8"))
    root = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    crate = tomllib.loads(
        (ROOT / "crates/truco-server/Cargo.toml").read_text(encoding="utf-8")
    )
    workspace_dependencies = root["workspace"]["dependencies"]
    crate_dependencies = crate["dependencies"]
    failures: list[str] = []

    if lock.get("format") != "baixada-server-contract-lock/v1":
        failures.append("contracts.lock.json has an unsupported format")

    for group_name in ("engine", "bots"):
        group = lock.get(group_name, {})
        for package in group.get("packages", []):
            dependency = workspace_dependencies.get(package, {})
            if dependency.get("git") != group.get("repository"):
                failures.append(f"{package} Git URL differs from the contract lock")
            if dependency.get("rev") != group.get("revision"):
                failures.append(f"{package} revision differs from the contract lock")
            if crate_dependencies.get(package) != {"workspace": True}:
                failures.append(
                    f"crates/truco-server must inherit {package} from the workspace"
                )

    cargo_lock = (ROOT / "Cargo.lock").read_text(encoding="utf-8")
    for group_name in ("engine", "bots"):
        group = lock[group_name]
        expected_source = (
            f'git+{group["repository"]}?rev={group["revision"]}'
            f'#{group["revision"]}'
        )
        if expected_source not in cargo_lock:
            failures.append(f"Cargo.lock does not contain exact {group_name} revision")

    if failures:
        for failure in failures:
            print(f"ERROR: {failure}")
        return 1
    print("Engine and bots dependencies match contracts.lock.json.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
