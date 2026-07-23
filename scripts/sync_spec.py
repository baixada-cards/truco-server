"""Materialize and verify the exact truco-spec revision pinned by this repo."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
LOCK_PATH = ROOT / "spec.lock.json"
DEFAULT_DESTINATION = ROOT / ".cache" / "truco-spec"
RELEASE_PATHS = (
    "LICENSE",
    "README.md",
    "VERSION",
    "spec-manifest.json",
    "rulesets",
    "engine",
    "exploration",
    "notation",
    "schemas",
)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_lock() -> dict[str, str]:
    payload = json.loads(LOCK_PATH.read_text(encoding="utf-8"))
    required = {
        "format",
        "repository",
        "revision",
        "spec_manifest_sha256",
        "version",
    }
    if set(payload) != required or payload["format"] != "truco-spec-lock/v1":
        raise ValueError("spec.lock.json has an unsupported shape")
    if len(payload["revision"]) != 40:
        raise ValueError("spec.lock.json revision must be a full commit SHA")
    return payload


def validate_source(source: Path, lock: dict[str, str]) -> None:
    version = (source / "VERSION").read_text(encoding="utf-8").strip()
    if version != lock["version"]:
        raise ValueError(f"expected truco-spec {lock['version']}, found {version}")

    manifest_path = source / "spec-manifest.json"
    actual_manifest_digest = sha256(manifest_path)
    if actual_manifest_digest != lock["spec_manifest_sha256"]:
        raise ValueError(
            "truco-spec manifest digest differs from spec.lock.json: "
            f"{actual_manifest_digest}"
        )

    manifest: dict[str, Any] = json.loads(manifest_path.read_text(encoding="utf-8"))
    if (
        manifest.get("format") != "truco-spec-manifest/v1"
        or manifest.get("version") != lock["version"]
    ):
        raise ValueError("truco-spec manifest version or format is invalid")

    seen: set[str] = set()
    for entry in manifest.get("files", []):
        path_text = entry.get("path")
        if not isinstance(path_text, str) or path_text in seen:
            raise ValueError("truco-spec manifest contains an invalid file entry")
        seen.add(path_text)
        relative = PurePosixPath(path_text)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError(
                f"truco-spec manifest contains an unsafe path: {path_text}"
            )
        path = source.joinpath(*relative.parts)
        if path.is_symlink() or not path.is_file():
            raise ValueError(
                f"truco-spec payload file is missing or unsafe: {path_text}"
            )
        if path.stat().st_size != entry.get("bytes") or sha256(path) != entry.get(
            "sha256"
        ):
            raise ValueError(f"truco-spec payload digest mismatch: {path_text}")

    for relative_text in RELEASE_PATHS:
        release_path = source / relative_text
        paths = release_path.rglob("*") if release_path.is_dir() else (release_path,)
        for path in paths:
            if path.is_symlink():
                raise ValueError(
                    f"truco-spec release tree must not contain symlinks: "
                    f"{path.relative_to(source)}"
                )


def clone_revision(repository: str, revision: str, destination: Path) -> None:
    commands = (
        ["git", "init", "--quiet", str(destination)],
        ["git", "-C", str(destination), "remote", "add", "origin", repository],
        [
            "git",
            "-C",
            str(destination),
            "fetch",
            "--quiet",
            "--depth=1",
            "--filter=blob:none",
            "origin",
            revision,
        ],
        [
            "git",
            "-C",
            str(destination),
            "checkout",
            "--quiet",
            "--detach",
            "FETCH_HEAD",
        ],
    )
    for command in commands:
        subprocess.run(command, check=True, text=True)

    actual = subprocess.run(
        ["git", "-C", str(destination), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if actual != revision:
        raise ValueError(f"expected truco-spec revision {revision}, fetched {actual}")


def copy_release(source: Path, destination: Path, lock: dict[str, str]) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=".truco-spec-sync-", dir=destination.parent
    ) as temporary_name:
        staged = Path(temporary_name) / "payload"
        staged.mkdir()
        for relative_text in RELEASE_PATHS:
            source_path = source / relative_text
            destination_path = staged / relative_text
            if source_path.is_dir():
                shutil.copytree(source_path, destination_path)
            else:
                shutil.copy2(source_path, destination_path)
        (staged / ".source.json").write_text(
            json.dumps(lock, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        validate_source(staged, lock)
        if destination.exists():
            shutil.rmtree(destination)
        staged.replace(destination)


def is_current(destination: Path, lock: dict[str, str]) -> bool:
    try:
        recorded = json.loads(
            (destination / ".source.json").read_text(encoding="utf-8")
        )
        if recorded != lock:
            return False
        validate_source(destination, lock)
    except (FileNotFoundError, ValueError, json.JSONDecodeError):
        return False
    return True


def sync(
    *,
    source: Path | None,
    destination: Path,
    check_only: bool,
) -> None:
    lock = load_lock()
    if is_current(destination, lock):
        print(
            f"truco-spec {lock['version']} ({lock['revision'][:12]}) verified at "
            f"{destination}"
        )
        return
    if check_only:
        raise SystemExit(
            f"{destination} is absent or does not match spec.lock.json; "
            "run `make sync-spec`"
        )

    configured_source = source or (
        Path(value) if (value := os.environ.get("TRUCO_SPEC_SOURCE")) else None
    )
    if configured_source is not None:
        configured_source = configured_source.expanduser().resolve()
        validate_source(configured_source, lock)
        copy_release(configured_source, destination, lock)
    else:
        destination.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(
            prefix=".truco-spec-clone-", dir=destination.parent
        ) as clone_name:
            clone = Path(clone_name)
            clone_revision(lock["repository"], lock["revision"], clone)
            validate_source(clone, lock)
            copy_release(clone, destination, lock)

    print(
        f"truco-spec {lock['version']} ({lock['revision'][:12]}) synced to "
        f"{destination}"
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--source",
        type=Path,
        help="verified local truco-spec checkout; otherwise fetch the pinned commit",
    )
    parser.add_argument(
        "--destination",
        type=Path,
        default=DEFAULT_DESTINATION,
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify the existing materialized contract without changing it",
    )
    args = parser.parse_args()
    try:
        sync(
            source=args.source,
            destination=args.destination.resolve(),
            check_only=args.check,
        )
    except (FileNotFoundError, ValueError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    else:
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
