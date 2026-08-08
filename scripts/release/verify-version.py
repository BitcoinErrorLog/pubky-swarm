#!/usr/bin/env python3
import json
import pathlib
import re
import sys


def fail(message: str) -> None:
    print(f"release blocked: {message}", file=sys.stderr)
    raise SystemExit(1)


if len(sys.argv) != 2:
    print(f"usage: {sys.argv[0]} vMAJOR.MINOR.PATCH", file=sys.stderr)
    raise SystemExit(64)

tag = sys.argv[1]
match = re.fullmatch(
    r"v(?P<version>[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)",
    tag,
)
if match is None:
    fail(f"invalid semantic version tag {tag!r}")

expected = match.group("version")
root = pathlib.Path(__file__).resolve().parents[2]

cargo_text = (root / "Cargo.toml").read_text(encoding="utf-8")
workspace_package = re.search(
    r"(?ms)^\[workspace\.package\]\s*(.*?)(?=^\[|\Z)",
    cargo_text,
)
if workspace_package is None:
    fail("Cargo.toml has no [workspace.package] section")
cargo_version = re.search(
    r'(?m)^version\s*=\s*"([^"]+)"\s*$',
    workspace_package.group(1),
)
if cargo_version is None:
    fail("Cargo.toml has no workspace package version")

package_version = json.loads(
    (root / "apps/desktop/package.json").read_text(encoding="utf-8")
)["version"]
tauri_version = json.loads(
    (root / "apps/desktop/src-tauri/tauri.conf.json").read_text(encoding="utf-8")
)["version"]

versions = {
    "Cargo.toml workspace package": cargo_version.group(1),
    "apps/desktop/package.json": package_version,
    "apps/desktop/src-tauri/tauri.conf.json": tauri_version,
}
wrong = {source: version for source, version in versions.items() if version != expected}
if wrong:
    details = ", ".join(f"{source}={version}" for source, version in wrong.items())
    fail(f"tag {tag} does not match application versions: {details}")

print(f"release versions match {tag}")
