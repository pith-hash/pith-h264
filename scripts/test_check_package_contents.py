"""Offline tests for check-package-contents.py: the gate must read the archive's
own entries, so a package that installs but lacks a platform's native library
has to fail, and a glob that matches nothing must never pass vacuously.

Run: python -m unittest scripts/test_check_package_contents.py
"""
from __future__ import annotations

import importlib.util
import io
import pathlib
import tarfile
import tempfile
import unittest
import zipfile

_spec = importlib.util.spec_from_file_location(
    "check_package_contents", pathlib.Path(__file__).with_name("check-package-contents.py")
)
cpc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cpc)

ALL = ["linux-x64", "linux-arm64", "darwin-arm64", "darwin-x64", "win32-x64"]


def make_zip(path: pathlib.Path, names: list[str]) -> pathlib.Path:
    with zipfile.ZipFile(path, "w") as z:
        for n in names:
            z.writestr(n, b"x")
    return path


def make_tgz(path: pathlib.Path, names: list[str]) -> pathlib.Path:
    with tarfile.open(path, "w:gz") as t:
        for n in names:
            info = tarfile.TarInfo(n)
            info.size = 1
            t.addfile(info, io.BytesIO(b"x"))
    return path


class NamesTests(unittest.TestCase):
    def test_cdylib_names(self):
        self.assertEqual(cpc.cdylib_name("win32-x64", "pith_x"), "pith_x.dll")
        self.assertEqual(cpc.cdylib_name("darwin-arm64", "pith_x"), "libpith_x.dylib")
        self.assertEqual(cpc.cdylib_name("linux-arm64", "pith_x"), "libpith_x.so")

    def test_unknown_platform_raises(self):
        with self.assertRaises(ValueError):
            cpc.cdylib_name("freebsd-x64", "pith_x")
        with self.assertRaises(ValueError):
            cpc.wheel_platform_patterns("freebsd-x64")


class WheelTests(unittest.TestCase):
    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp())

    def wheel(self, tag: str, entry: str | None = "pith_x/libpith_x.so"):
        return make_zip(
            self.tmp / f"pith_x-0.1.1-{tag}.whl",
            [entry] if entry else ["pith_x/__init__.py"],
        )

    def test_all_platforms_present_passes(self):
        wheels = [
            self.wheel("py3-none-manylinux_2_17_x86_64"),
            self.wheel("py3-none-manylinux_2_17_aarch64"),
            self.wheel("py3-none-macosx_11_0_arm64", "pith_x/libpith_x.dylib"),
            self.wheel("py3-none-macosx_10_12_x86_64", "pith_x/libpith_x.dylib"),
            self.wheel("py3-none-win_amd64", "pith_x/pith_x.dll"),
        ]
        self.assertEqual(cpc.check_wheels(wheels, ALL, "pith_x"), [])

    def test_missing_platform_fails(self):
        wheels = [self.wheel("py3-none-manylinux_2_17_x86_64")]
        got = cpc.check_wheels(wheels, ["linux-x64", "win32-x64"], "pith_x")
        self.assertTrue(any("win32-x64" in p for p in got))

    def test_abi_specific_wheel_fails(self):
        wheels = [self.wheel("cp313-cp313-win_amd64", "pith_x/pith_x.dll")]
        got = cpc.check_wheels(wheels, ["win32-x64"], "pith_x")
        self.assertTrue(any("py3-none" in p for p in got))

    def test_wheel_without_cdylib_fails(self):
        wheels = [self.wheel("py3-none-win_amd64", None)]
        got = cpc.check_wheels(wheels, ["win32-x64"], "pith_x")
        self.assertTrue(any("pith_x.dll" in p for p in got))

    def test_zero_wheels_fails(self):
        self.assertTrue(cpc.check_wheels([], ["win32-x64"], "pith_x"))


class JarTests(unittest.TestCase):
    def test_complete_jar_passes(self):
        tmp = pathlib.Path(tempfile.mkdtemp())
        names = [f"hash/pith/native/{p}/{cpc.cdylib_name(p, 'pith_x')}" for p in ALL]
        jar = make_zip(tmp / "pith-x-0.1.1.jar", names + ["hash/pith/PithNative.class"])
        self.assertEqual(cpc.check_jar(jar, ALL, "pith_x", "hash/pith/native"), [])

    def test_thin_jar_fails_per_platform(self):
        tmp = pathlib.Path(tempfile.mkdtemp())
        jar = make_zip(tmp / "thin.jar", ["hash/pith/PithNative.class"])
        got = cpc.check_jar(jar, ["linux-x64", "win32-x64"], "pith_x", "hash/pith/native")
        self.assertEqual(len(got), 2)

    def test_missing_file_fails(self):
        got = cpc.check_jar(
            pathlib.Path("/nonexistent.jar"), ["linux-x64"], "pith_x", "hash/pith/native"
        )
        self.assertTrue(got)


class NpmTests(unittest.TestCase):
    def test_complete_tarball_passes(self):
        tmp = pathlib.Path(tempfile.mkdtemp())
        names = [f"package/prebuilds/{p}/{cpc.cdylib_name(p, 'pith_x')}" for p in ALL]
        tgz = make_tgz(tmp / "x.tgz", names + ["package/package.json"])
        self.assertEqual(cpc.check_npm(tgz, ALL, "pith_x"), [])

    def test_missing_prebuild_fails(self):
        tmp = pathlib.Path(tempfile.mkdtemp())
        tgz = make_tgz(tmp / "x.tgz", ["package/prebuilds/linux-x64/libpith_x.so"])
        got = cpc.check_npm(tgz, ["linux-x64", "darwin-x64"], "pith_x")
        self.assertEqual(len(got), 1)
        self.assertIn("darwin-x64", got[0])

    def test_missing_file_fails(self):
        self.assertTrue(cpc.check_npm(pathlib.Path("/nonexistent.tgz"), ["linux-x64"], "pith_x"))


class CliTests(unittest.TestCase):
    def test_glob_matching_nothing_exits_1(self):
        rc = cpc.main(
            ["wheels", "/nonexistent-dir/*.whl", "--platforms", "win32-x64", "--stem", "pith_x"]
        )
        self.assertEqual(rc, 1)

    def test_bad_platform_exits_2(self):
        with self.assertRaises(SystemExit) as cm:
            cpc.main(["wheels", "x", "--platforms", "freebsd-x64", "--stem", "pith_x"])
        self.assertEqual(cm.exception.code, 2)

    def test_cli_end_to_end_on_jar_and_npm(self):
        tmp = pathlib.Path(tempfile.mkdtemp())
        make_zip(tmp / "a.jar", ["hash/pith/native/linux-x64/libpith_x.so"])
        make_tgz(tmp / "a.tgz", ["package/prebuilds/linux-x64/libpith_x.so"])
        base = ["--platforms", "linux-x64", "--stem", "pith_x"]
        self.assertEqual(cpc.main(["jar", str(tmp / "*.jar")] + base), 0)
        self.assertEqual(cpc.main(["npm", str(tmp / "*.tgz")] + base), 0)
        self.assertEqual(
            cpc.main(["jar", str(tmp / "*.jar"), "--platforms", "win32-x64", "--stem", "pith_x"]), 1
        )


if __name__ == "__main__":
    unittest.main()
