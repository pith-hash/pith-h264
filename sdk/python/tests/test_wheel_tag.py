# SPDX-License-Identifier: MIT
# Copyright (c) 2026 pith-hash
"""The ctypes wheel must be tagged py3-none-<platform>, never cpXY-cpXY-<platform>.

The cdylib is loaded through ctypes and never links the Python C API, so a
wheel pinned to the building interpreter's ABI only installs on one CPython
minor version for no reason.
"""

import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SDK = Path(__file__).resolve().parent.parent


class WheelTagTests(unittest.TestCase):
    def test_wheel_is_py3_none_platform(self):
        with tempfile.TemporaryDirectory() as lib, tempfile.TemporaryDirectory() as work:
            # setup.py scans PITH_CDYLIB_DIR for any of the three cdylib names.
            (Path(lib) / "pith_h264.dll").write_bytes(b"MZ")
            out = Path(work) / "dist"
            subprocess.run(
                [
                    sys.executable, "setup.py", "-q",
                    "build", "--build-base", str(Path(work) / "build"),
                    "egg_info", "--egg-base", work,
                    "bdist_wheel", "--dist-dir", str(out),
                    "--bdist-dir", str(Path(work) / "bdist"),
                ],
                cwd=SDK,
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
