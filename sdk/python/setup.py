# SPDX-License-Identifier: MIT
# Copyright (c) 2026 pith-hash
"""Build backend glue for the pith-h264 ctypes wheel.

The wheel carries the Rust cdylib built by ``cargo build --release``:
``PITH_CDYLIB_DIR`` (default ``target/release`` at the repository root)
is scanned for the cdylib and copied into the wheel by a ``build_py``
hook. ``BinaryDistribution`` marks the distribution non-pure so the
wheel is tagged per platform — the CD matrix builds one wheel per OS
and publish-pypi uploads the three together.
"""

from __future__ import annotations

import os
import shutil
from pathlib import Path

from setuptools import Distribution, setup
from setuptools.command.build_py import build_py

#: Every cdylib file name cargo may drop into the build directory, per
#: platform (windows / linux / macOS).
CDYLIB_NAMES = ("pith_h264.dll", "libpith_h264.so", "libpith_h264.dylib")


class BinaryDistribution(Distribution):
    """A distribution that always carries a binary, so wheels get a
    platform tag instead of ``py3-none-any``."""

    def has_ext_modules(self) -> bool:  # noqa: D102
        return True


class build_py_with_cdylib(build_py):
    """Copies the cdylib from ``PITH_CDYLIB_DIR`` into the wheel."""

    def run(self) -> None:
        super().run()
        repo_root = Path(__file__).resolve().parent.parent.parent
        env_dir = os.environ.get("PITH_CDYLIB_DIR")
        candidates = [Path(env_dir)] if env_dir else []
        if not env_dir or not Path(env_dir).is_absolute():
            # Relative values are ambiguous: build backends run with the
            # package directory as cwd, while CD and local shells point the
            # variable at the repository-root-relative target/release.
            candidates.append(repo_root / (env_dir or "target/release"))
        candidates.append(repo_root / "target" / "release")
        pkg_dir = Path(self.build_lib) / "pith_h264"
        copied = 0
        for candidate in candidates:
            for name in CDYLIB_NAMES:
                src = candidate / name
                if src.is_file():
                    self.mkpath(str(pkg_dir))
                    shutil.copy2(src, pkg_dir / name)
                    copied += 1
            if copied:
                break
        if copied == 0:
            raise SystemExit(
                f"no cdylib (any of {', '.join(CDYLIB_NAMES)}) found in "
                f"{', '.join(str(c) for c in candidates)}; run `cargo build --release` first"
            )


setup(
    cmdclass={"build_py": build_py_with_cdylib},
    distclass=BinaryDistribution,
)
