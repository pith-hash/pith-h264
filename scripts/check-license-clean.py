#!/usr/bin/env python3
"""Licence hygiene gate for the pith suite.

Every pith-* repository ships the same plain MIT text in LICENSE. Anything
extra in that file, such as an upstream attribution paragraph, stops GitHub
from classifying the licence (the API then reports NOASSERTION instead of MIT).
Upstream attribution belongs in NOTICE, which every repository must carry.

The gate fails when:
  - LICENSE is missing or differs from the canonical text below, or
  - NOTICE is missing or empty.

A CRLF checkout of the canonical text is accepted; the repositories store LF.

Usage: python scripts/check-license-clean.py [--repo-root DIR]
"""
from __future__ import annotations

import argparse
import pathlib
import sys

CANONICAL_LICENSE = """\
MIT License

Copyright (c) 2026 pith-hash contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""


def check(root: pathlib.Path) -> list[str]:
    """Return one message per problem found under `root`; empty means clean."""
    problems: list[str] = []

    license_path = root / "LICENSE"
    if not license_path.is_file():
        problems.append("LICENSE is missing")
    else:
        text = license_path.read_bytes().decode("utf-8", errors="replace")
        if text.replace("\r\n", "\n") != CANONICAL_LICENSE:
            problems.append(
                "LICENSE differs from the canonical MIT text; move any upstream "
                "attribution into NOTICE and restore LICENSE verbatim"
            )

    notice_path = root / "NOTICE"
    if not notice_path.is_file():
        problems.append("NOTICE is missing")
    elif not notice_path.read_bytes().strip():
        problems.append("NOTICE is empty")

    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--repo-root",
        type=pathlib.Path,
        default=pathlib.Path(__file__).resolve().parents[1],
        help="repository checkout to check (default: this repository)",
    )
    args = parser.parse_args(argv)

    problems = check(args.repo_root)
    if problems:
        print("FAIL: licence hygiene:")
        for problem in problems:
            print(f"  - {problem}")
        return 1

    print("licence hygiene OK: LICENSE is the canonical MIT text and NOTICE is present")
    return 0


if __name__ == "__main__":
    sys.exit(main())
