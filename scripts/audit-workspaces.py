#!/usr/bin/env python3
"""Keep the engine and bundled-mod workspace policies in sync."""

from pathlib import Path
import sys
import tomllib


ROOT = Path(__file__).resolve().parent.parent


def manifest(path: Path) -> dict:
    with path.open("rb") as stream:
        return tomllib.load(stream)


def dependency(value: object, workspace: Path) -> object:
    if isinstance(value, str):
        return {"version": value}
    if isinstance(value, dict):
        normalized = value.copy()
        if "path" in normalized:
            normalized["path"] = str((workspace / normalized["path"]).resolve())
        return normalized
    return value


def main() -> int:
    engine = manifest(ROOT / "Cargo.toml")["workspace"]
    mods = manifest(ROOT / "mods-src/Cargo.toml")["workspace"]
    errors = []

    for key in ("edition", "rust-version", "license"):
        if engine["package"][key] != mods["package"][key]:
            errors.append(f"workspace.package.{key} differs")

    if engine["lints"] != mods["lints"]:
        errors.append("workspace.lints differs")

    shared = engine["dependencies"].keys() & mods["dependencies"].keys()
    for name in sorted(shared):
        root_dep = dependency(engine["dependencies"][name], ROOT)
        mod_dep = dependency(mods["dependencies"][name], ROOT / "mods-src")
        if root_dep != mod_dep:
            errors.append(f"workspace.dependencies.{name} differs")

    for error in errors:
        print(f"source audit: {error}", file=sys.stderr)
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
