#!/usr/bin/env python3
"""Zero-third-party-dependency gate for the pith suite (modhash company-split D1).

Every dependency of every workspace member must be a `pith-*` crate: normal,
build and dev dependencies alike. The suite contract is std-only Rust with
`forbid(unsafe_code)`; any third-party crate in the graph fails this gate.

Also asserts the suite naming rule: every workspace member is itself named
`pith-<domain>`.

Usage: python scripts/check-zero-deps.py
"""
from __future__ import annotations

import json
import subprocess
import sys

ALLOWED_PREFIX = "pith-"


def main() -> int:
    try:
        proc = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--no-deps"],
            capture_output=True,
            text=True,
        )
    except FileNotFoundError:
        print("FAIL: cargo not found on PATH; run this gate where cargo is installed")
        return 1
    if proc.returncode != 0:
        print("FAIL: cargo metadata failed:")
        print(proc.stderr.strip())
        return 1

    data = json.loads(proc.stdout)
    member_ids = set(data["workspace_members"])

    violations: list[str] = []

    members = [pkg for pkg in data["packages"] if pkg["id"] in member_ids]
    for pkg in members:
        if not pkg["name"].startswith(ALLOWED_PREFIX):
            violations.append(
                f"workspace member {pkg['name']!r} violates the suite naming rule "
                f"(must start with {ALLOWED_PREFIX!r})"
            )
        for dep in pkg["dependencies"]:
            kind = dep.get("kind") or "normal"
            if dep["name"] == pkg["name"]:
                continue
            if not dep["name"].startswith(ALLOWED_PREFIX):
                violations.append(
                    f"{pkg['name']}: {kind} dependency {dep['name']!r} "
                    f"(req {dep.get('req', '?')}) is outside the pith-* whitelist"
                )

    if violations:
        print(f"FAIL: {len(violations)} dependency violation(s):")
        for line in violations:
            print(f"  - {line}")
        return 1

    print(
        f"zero-dep gate OK: {len(members)} workspace member(s), "
        "all dependencies are pith-*"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
