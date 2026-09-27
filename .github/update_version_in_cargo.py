#!/usr/bin/env python3
"""Set the git-uplink package version in Cargo.toml and Cargo.lock.

Rewrites only the root package version. Dependency versions stay as they are,
so this does not resolve crates or touch the network.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PACKAGE_NAME = "git-uplink"
VERSION_RE = re.compile(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?\Z")
TOML_VERSION_RE = re.compile(r'^version = "[^"]*"$', re.MULTILINE)
LOCK_NAME_RE = re.compile(r'^name = "([^"]*)"[ \t]*$')
LOCK_VERSION_RE = re.compile(r'^version = "[^"]*"[ \t]*$')


def read_text(path: Path) -> str:
    with path.open(encoding="utf-8", newline="") as handle:
        return handle.read()


def write_text(path: Path, text: str) -> None:
    with path.open("w", encoding="utf-8", newline="") as handle:
        handle.write(text)


def update_cargo_toml(text: str, version: str) -> str:
    matches = list(TOML_VERSION_RE.finditer(text))
    if len(matches) != 1:
        raise SystemExit(
            f"Cargo.toml: expected one package version line, found {len(matches)}"
        )
    match = matches[0]
    return text[: match.start()] + f'version = "{version}"' + text[match.end() :]


def update_cargo_lock(text: str, version: str) -> str:
    lines = text.splitlines(keepends=True)
    starts = [index for index, line in enumerate(lines) if line.strip() == "[[package]]"]
    replacements = []
    for index, start in enumerate(starts):
        end = starts[index + 1] if index + 1 < len(starts) else len(lines)
        name = None
        version_index = None
        for offset, line in enumerate(lines[start:end]):
            name_match = LOCK_NAME_RE.match(line.rstrip("\r\n"))
            if name_match and name is None:
                name = name_match.group(1)
            elif LOCK_VERSION_RE.match(line.rstrip("\r\n")) and version_index is None:
                version_index = start + offset
        if name == PACKAGE_NAME:
            if version_index is None:
                raise SystemExit("Cargo.lock: git-uplink package has no version line")
            replacements.append(version_index)
    if len(replacements) != 1:
        raise SystemExit(
            f"Cargo.lock: expected one {PACKAGE_NAME} package, found {len(replacements)}"
        )
    version_index = replacements[0]
    line = lines[version_index]
    ending = ""
    if line.endswith("\r\n"):
        ending = "\r\n"
    elif line.endswith("\n"):
        ending = "\n"
    lines[version_index] = f'version = "{version}"{ending}'
    return "".join(lines)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {Path(sys.argv[0]).name} <version>")
    version = sys.argv[1]
    if VERSION_RE.fullmatch(version) is None:
        raise SystemExit(f"invalid version: {version!r}")

    toml_path = ROOT / "Cargo.toml"
    lock_path = ROOT / "Cargo.lock"
    toml_text = update_cargo_toml(read_text(toml_path), version)
    lock_text = update_cargo_lock(read_text(lock_path), version)
    write_text(toml_path, toml_text)
    write_text(lock_path, lock_text)


if __name__ == "__main__":
    main()
