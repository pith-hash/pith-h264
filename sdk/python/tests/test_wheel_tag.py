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
import unittest
from pathlib import Path

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


if __name__ == "__main__":
    unittest.main()
