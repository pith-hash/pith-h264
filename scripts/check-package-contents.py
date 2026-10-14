#!/usr/bin/env python3
"""Package-contents gate: a published artifact must carry a native library for
every platform it advertises.

Exact-version presence on a registry proves nothing about usability: a JAR
without its cdylib, a wheel set missing a platform, or an npm tarball missing a
prebuild all install and then fail at load time, and a version cannot be
replaced. This gate only lists archive entries (nothing from an archive is
executed) and exits non-zero when anything is missing, so it can sit in front
of every publish job.

  check-package-contents.py wheels 'wheelhouse-flat/*.whl' --platforms linux-x64,win32-x64 --stem pith_x
  check-package-contents.py jar    'sdk/java/target/*.jar' --platforms ... --stem pith_x --prefix hash/pith/native
  check-package-contents.py npm    'sdk/node/*.tgz'        --platforms ... --stem pith_x

Stdlib only, like check-zero-deps.py and check-registries.py.
"""
from __future__ import annotations

import argparse
import fnmatch
import glob
import pathlib
import sys
import tarfile
import zipfile

#: Node-style platform id -> (cdylib file name format, wheel platform tag glob)
PLATFORMS: dict[str, tuple[str, str]] = {
    "linux-x64": ("lib{stem}.so", "manylinux*_x86_64"),
    "linux-arm64": ("lib{stem}.so", "manylinux*_aarch64"),
    "darwin-arm64": ("lib{stem}.dylib", "macosx_*_arm64"),
    "darwin-x64": ("lib{stem}.dylib", "macosx_*_x86_64"),
    "win32-x64": ("{stem}.dll", "win_amd64"),
}


def _known(platform: str) -> tuple[str, str]:
    if platform not in PLATFORMS:
        raise ValueError(f"unknown platform {platform!r}; known: {', '.join(sorted(PLATFORMS))}")
    return PLATFORMS[platform]


def cdylib_name(platform: str, stem: str) -> str:
    return _known(platform)[0].format(stem=stem)


def wheel_platform_patterns(platform: str) -> str:
    return "py3-none-" + _known(platform)[1]


def _wheel_tag(path: pathlib.Path) -> str:
    # {name}-{version}-{python}-{abi}-{platform}.whl -> "{python}-{abi}-{platform}"
    return "-".join(path.name[: -len(".whl")].split("-")[-3:])


def check_wheels(wheels: list[pathlib.Path], platforms: list[str], stem: str) -> list[str]:
    if not wheels:
        return ["no wheel files found"]
    problems: list[str] = []
    for w in wheels:
        tag = _wheel_tag(w)
        if not tag.startswith("py3-none-"):
            problems.append(
                f"{w.name}: tag {tag!r} is not py3-none-<platform> "
                "(an interpreter-specific wheel only installs on one Python)"
            )
    for platform in platforms:
        pattern = wheel_platform_patterns(platform)
        matching = [w for w in wheels if fnmatch.fnmatchcase(_wheel_tag(w), pattern)]
        if not matching:
            problems.append(f"{platform}: no wheel matches {pattern}")
            continue
        want = cdylib_name(platform, stem)
        for w in matching:
            with zipfile.ZipFile(w) as z:
                if not any(n.rsplit("/", 1)[-1] == want for n in z.namelist()):
                    problems.append(f"{w.name}: does not contain {want}")
    return problems


def check_jar(jar: pathlib.Path, platforms: list[str], stem: str, prefix: str) -> list[str]:
    if not jar.is_file():
        return [f"{jar}: not a file"]
    with zipfile.ZipFile(jar) as z:
        names = set(z.namelist())
    problems = []
    for platform in platforms:
        entry = f"{prefix.strip('/')}/{platform}/{cdylib_name(platform, stem)}"
        if entry not in names:
            problems.append(f"{jar.name}: missing {entry}")
    return problems


def check_npm(tgz: pathlib.Path, platforms: list[str], stem: str) -> list[str]:
    if not tgz.is_file():
        return [f"{tgz}: not a file"]
    with tarfile.open(tgz, "r:gz") as t:
        names = set(t.getnames())
    problems = []
    for platform in platforms:
        entry = f"package/prebuilds/{platform}/{cdylib_name(platform, stem)}"
        if entry not in names:
            problems.append(f"{tgz.name}: missing {entry}")
    return problems


def _expand(patterns: list[str]) -> list[pathlib.Path]:
    return sorted({pathlib.Path(p) for pat in patterns for p in glob.glob(pat)})


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("kind", choices=["wheels", "jar", "npm"])
    ap.add_argument("paths", nargs="+", help="files or globs")
    ap.add_argument("--platforms", required=True, help="comma-separated platform ids")
    ap.add_argument("--stem", required=True, help="cdylib stem, e.g. pith_hash")
    ap.add_argument("--prefix", default="hash/pith/native", help="jar resource prefix")
    args = ap.parse_args(argv)
    platforms = [p.strip() for p in args.platforms.split(",") if p.strip()]
    unknown = [p for p in platforms if p not in PLATFORMS]
    if unknown:
        ap.error(f"unknown platform: {', '.join(unknown)}")
    files = _expand(args.paths)
    if not files:
        print(f"FAIL: nothing matched {args.paths}")
        return 1
    if args.kind == "wheels":
        problems = check_wheels(files, platforms, args.stem)
    else:
        problems = []
        for f in files:
            problems += (
                check_jar(f, platforms, args.stem, args.prefix)
                if args.kind == "jar"
                else check_npm(f, platforms, args.stem)
            )
    for p in problems:
        print(f"FAIL: {p}")
    if not problems:
        print(f"ok: {len(files)} {args.kind} file(s) carry {', '.join(platforms)}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
