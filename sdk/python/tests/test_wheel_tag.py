# SPDX-License-Identifier: MIT
# Copyright (c) 2026 pith-hash
"""The ctypes wheel must be tagged py3-none-<platform>, never cpXY-cpXY-<platform>.

The cdylib is loaded through ctypes and never links the Python C API, so a
wheel pinned to the building interpreter's ABI only installs on one CPython
minor version for no reason.
"""

import os
import re
import shutil
import subprocess
import sys
import tempfile
import types
import unittest
import importlib.util
from pathlib import Path
from unittest import mock

SDK = Path(__file__).resolve().parent.parent


class WheelTagTests(unittest.TestCase):
    def test_wheel_is_py3_none_platform(self):
        # Build through pip, as the CD does: the isolated build env brings the
        # setuptools pinned in pyproject.toml, so the host interpreter needs no
        # setuptools of its own (CPython 3.12+ venvs ship without it).
        with tempfile.TemporaryDirectory() as lib, tempfile.TemporaryDirectory() as work:
            # setup.py scans PITH_CDYLIB_DIR for any of the three cdylib names.
            (Path(lib) / "pith_h264.dll").write_bytes(b"MZ")
            source = Path(work) / "sdk"
            shutil.copytree(
                SDK, source,
                ignore=shutil.ignore_patterns("build", "dist", "*.egg-info", "__pycache__", ".pytest_cache"),
            )
            out = Path(work) / "dist"
            subprocess.run(
                [sys.executable, "-m", "pip", "wheel", "--no-deps", "-q", "-w", str(out), str(source)],
                env={**os.environ, "PITH_CDYLIB_DIR": lib},
                check=True,
            )
            names = os.listdir(out)
            self.assertEqual(len(names), 1, names)
            self.assertRegex(
                names[0],
                re.compile(r"^pith_h264-\d+\.\d+\.\d+.*-py3-none-(?!any\.)[A-Za-z0-9_]+\.whl$"),
            )


def _load_setup_module():
    """Import setup.py without setuptools: the host interpreter may not ship it
    (CPython 3.12+ venvs do not), and only the pure tag helper is under test."""
    stubs = {}
    for name in ("setuptools", "setuptools.command", "setuptools.command.bdist_wheel", "setuptools.command.build_py"):
        stubs[name] = types.ModuleType(name)
    stubs["setuptools"].Distribution = type("Distribution", (), {})
    stubs["setuptools"].setup = lambda **kwargs: None
    stubs["setuptools.command.bdist_wheel"].bdist_wheel = type("bdist_wheel", (), {})
    stubs["setuptools.command.build_py"].build_py = type("build_py", (), {})
    spec = importlib.util.spec_from_file_location("pith_wheel_setup", SDK / "setup.py")
    module = importlib.util.module_from_spec(spec)
    with mock.patch.dict(sys.modules, stubs):
        spec.loader.exec_module(module)
    return module


class NativePlatformTagTests(unittest.TestCase):
    """The cdylib is single-architecture, so the wheel tag must be too."""

    @classmethod
    def setUpClass(cls):
        cls.tag = staticmethod(_load_setup_module().native_platform_tag)

    def test_universal2_becomes_the_host_arch(self):
        self.assertEqual(self.tag("macosx_11_0_universal2", "arm64"), "macosx_11_0_arm64")
        self.assertEqual(self.tag("macosx_10_13_universal2", "x86_64"), "macosx_10_13_x86_64")

    def test_other_platforms_are_untouched(self):
        for plat in ("macosx_11_0_arm64", "macosx_10_13_x86_64", "win_amd64", "linux_x86_64", "manylinux_2_28_aarch64"):
            self.assertEqual(self.tag(plat, "arm64"), plat)

    def test_universal2_on_an_unknown_arch_fails_loudly(self):
        with self.assertRaises(RuntimeError):
            self.tag("macosx_11_0_universal2", "ppc64")


if __name__ == "__main__":
    unittest.main()
