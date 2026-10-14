#!/usr/bin/env python3
"""Stamp one release version into every manifest the suite publishes from.

better-semantic-release computes the version and tags the release commit, but it
only rewrites files it is told about, and it cannot express a Cargo workspace, a
Cargo.lock, an npm lockfile or a pom.xml. Without this step the tag says 0.1.1
while sdk/node/package.json still says 0.1.0, the cd.yml version guard refuses the
tag, and (worse, were the guard absent) a registry would publish a version that
cannot be replaced. Run it from `build_command`; PSR stages only what it knows, so
this script `git add`s every file it changed.

  stamp-release.py [VERSION] [--root DIR] [--no-git-add]

VERSION defaults to $NEW_VERSION (what PSR exports to build_command). Stdlib only,
like the other scripts here. Files are edited in place, line endings preserved.
"""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import subprocess
import sys
import tomllib
import xml.etree.ElementTree as ET

SEMVER = re.compile(r"^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$")
#: Directories whose manifests are build output or somebody else's package.
PRUNE = {".git", "target", "node_modules", "dist", ".venv", "venv", "__pycache__"}
_TABLE = re.compile(r"^\s*\[([^\[\]]+)\]\s*(?:#.*)?$")
_VERSION_LINE = re.compile(r'^(\s*version\s*=\s*")([^"]*)(".*)$', re.S)


class StampError(Exception):
    """A manifest could not be stamped safely; nothing is half-written."""


def stamp_toml(text: str, table: str, version: str) -> tuple[str, str] | None:
    """Set `version = "..."` inside `[table]`; return (new_text, old_version) or None."""
    lines = text.splitlines(keepends=True)
    current = ""
    for i, line in enumerate(lines):
        header = _TABLE.match(line)
        if header:
            current = header.group(1).strip()
            continue
        if line.lstrip().startswith("[["):
            current = ""
            continue
        if current == table:
            m = _VERSION_LINE.match(line)
            if m:
                lines[i] = m.group(1) + version + m.group(3)
                return "".join(lines), m.group(2)
    return None


def stamp_cargo_lock(text: str, names: set[str], version: str) -> str:
    """Stamp workspace members in Cargo.lock; registry/git packages carry a `source`."""
    for name in sorted(names):
        pattern = re.compile(
            r'(\nname = "' + re.escape(name) + r'"\r?\nversion = ")[^"]*("\r?\n(?!source =))'
        )
        text, count = pattern.subn(lambda m: m.group(1) + version + m.group(2), text)
        if count != 1:
            raise StampError(f"Cargo.lock: expected exactly one workspace entry for {name}, found {count}")
    return text


def _dump_json(obj: object) -> str:
    return json.dumps(obj, indent=2, ensure_ascii=False) + "\n"


def _load_canonical_json(text: str, what: str) -> dict:
    obj = json.loads(text)
    # Re-serialising must reproduce the file byte for byte, otherwise stamping
    # would silently reformat it (and hide the real diff in review).
    if _dump_json(obj) != text:
        raise StampError(f"{what}: not canonical 2-space JSON; refusing to reformat it")
    return obj


def stamp_package_json(text: str, version: str) -> str:
    obj = _load_canonical_json(text, "package.json")
    obj["version"] = version
    return _dump_json(obj)


def stamp_package_lock(text: str, version: str) -> str:
    obj = _load_canonical_json(text, "package-lock.json")
    obj["version"] = version
    root = obj.get("packages", {}).get("")
    if root is not None:
        root["version"] = version
    return _dump_json(obj)


def stamp_pom(text: str, version: str) -> str:
    start = text.find("</parent>")
    if start < 0:
        start = text.find("</modelVersion>")
    if start < 0:
        raise StampError("pom.xml: no <modelVersion> to anchor on")
    pattern = re.compile(r"(</artifactId>\s*<version>)([^<]*)(</version>)")
    m = pattern.search(text, start)
    if not m:
        raise StampError("pom.xml: no <version> directly after the project <artifactId>")
    new = text[: m.start()] + m.group(1) + version + m.group(3) + text[m.end() :]
    root = ET.fromstring(new)
    ns = root.tag[: root.tag.index("}") + 1] if root.tag.startswith("{") else ""
    el = root.find(ns + "version")
    if el is None or (el.text or "").strip() != version:
        raise StampError("pom.xml: stamped a <version> that is not the project's own")
    return new


def _read(path: pathlib.Path) -> str:
    with open(path, encoding="utf-8", newline="") as f:
        return f.read()


def _write(path: pathlib.Path, text: str) -> None:
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(text)


def _cargo_manifests(root: pathlib.Path) -> list[pathlib.Path]:
    found = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in PRUNE)
        if "Cargo.toml" in filenames:
            found.append(pathlib.Path(dirpath) / "Cargo.toml")
    return found


def stamp_tree(root: pathlib.Path, version: str) -> list[pathlib.Path]:
    """Stamp every manifest under root; return the files that now differ."""
    changed: list[pathlib.Path] = []
    seen = 0

    def apply(path: pathlib.Path, new: str) -> None:
        if new != _read(path):
            _write(path, new)
            changed.append(path)

    member_names: set[str] = set()
    for manifest in _cargo_manifests(root):
        text = _read(manifest)
        data = tomllib.loads(text)
        for table, section in (("package", data.get("package")), ("workspace.package", data.get("workspace", {}).get("package"))):
            if not isinstance(section, dict) or not isinstance(section.get("version"), str):
                continue
            stamped = stamp_toml(text, table, version)
            if stamped is None:
                raise StampError(f"{manifest}: [{table}] has a version the line editor cannot find")
            text = stamped[0]
            seen += 1
            if table == "package":
                member_names.add(section["name"])
        apply(manifest, text)
    lock = root / "Cargo.lock"
    if lock.exists() and member_names:
        apply(lock, stamp_cargo_lock(_read(lock), member_names, version))

    py = root / "sdk" / "python" / "pyproject.toml"
    if py.exists():
        stamped = stamp_toml(_read(py), "project", version)
        if stamped is None:
            raise StampError(f"{py}: [project] has no literal version")
        seen += 1
        apply(py, stamped[0])
    for rel, fn in (
        ("sdk/node/package.json", stamp_package_json),
        ("sdk/node/package-lock.json", stamp_package_lock),
        ("sdk/java/pom.xml", stamp_pom),
    ):
        path = root / rel
        if path.exists():
            seen += 1
            apply(path, fn(_read(path), version))
    if seen == 0:
        raise StampError(f"no manifest to stamp under {root}")
    return changed


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("version", nargs="?", default=os.environ.get("NEW_VERSION", ""))
    ap.add_argument("--root", default=".", help="repository root")
    ap.add_argument("--no-git-add", action="store_true", help="do not stage the changed files")
    args = ap.parse_args(argv)
    if not SEMVER.match(args.version):
        ap.error(f"version {args.version!r} is not semver (no leading v); pass it or set NEW_VERSION")
    root = pathlib.Path(args.root).resolve()
    try:
        changed = stamp_tree(root, args.version)
    except StampError as exc:
        print(f"FAIL: {exc}")
        return 1
    for path in changed:
        print(f"stamped {path.relative_to(root).as_posix()} -> {args.version}")
    if not changed:
        print(f"ok: every manifest already carries {args.version}")
    if changed and not args.no_git_add:
        subprocess.run(["git", "add", "--", *map(str, changed)], cwd=root, check=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
