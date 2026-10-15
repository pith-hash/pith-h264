"""Offline tests for check-license-clean.py: the gate must fail when LICENSE is
not the canonical MIT text (an extra attribution paragraph is what stops
GitHub from classifying the licence) and when NOTICE, the file that carries
upstream attribution instead, is missing or empty.

Run: python -m unittest scripts/test_check_license_clean.py
"""
from __future__ import annotations

import contextlib
import importlib.util
import io
import pathlib
import tempfile
import unittest

_spec = importlib.util.spec_from_file_location(
    "check_license_clean", pathlib.Path(__file__).with_name("check-license-clean.py")
)
cl = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(cl)

NOTICE = "Portions derived from example/upstream (MIT), Copyright (c) 2026 example.\n"


class RepoCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = pathlib.Path(self._tmp.name)

    def write(self, name: str, text: str | bytes) -> None:
        data = text.encode() if isinstance(text, str) else text
        (self.root / name).write_bytes(data)

    def run_main(self) -> int:
        with contextlib.redirect_stdout(io.StringIO()):
            return cl.main(["--repo-root", str(self.root)])

    def clean_repo(self) -> None:
        self.write("LICENSE", cl.CANONICAL_LICENSE)
        self.write("NOTICE", NOTICE)


class CanonicalTextTests(unittest.TestCase):
    def test_is_plain_mit_with_suite_holder(self):
        text = cl.CANONICAL_LICENSE
        self.assertTrue(text.startswith("MIT License\n\nCopyright (c) 2026 pith-hash contributors\n"))
        self.assertTrue(text.endswith("SOFTWARE.\n"))
        self.assertNotIn("\r", text)
        self.assertNotIn("Portions derived", text)

    def test_length_matches_the_published_licence(self):
        self.assertEqual(len(cl.CANONICAL_LICENSE.encode()), 1079)


class CheckTests(RepoCase):
    def test_clean_repo_passes(self):
        self.clean_repo()
        self.assertEqual(cl.check(self.root), [])
        self.assertEqual(self.run_main(), 0)

    def test_crlf_checkout_of_canonical_text_passes(self):
        self.clean_repo()
        self.write("LICENSE", cl.CANONICAL_LICENSE.replace("\n", "\r\n"))
        self.assertEqual(cl.check(self.root), [])

    def test_missing_license_fails(self):
        self.clean_repo()
        (self.root / "LICENSE").unlink()
        problems = cl.check(self.root)
        self.assertEqual(len(problems), 1)
        self.assertIn("LICENSE is missing", problems[0])
        self.assertEqual(self.run_main(), 1)

    def test_attribution_paragraph_in_license_fails(self):
        self.clean_repo()
        self.write(
            "LICENSE",
            cl.CANONICAL_LICENSE
            + "\n---\n\nPortions derived from n24q02m/modhash (MIT), Copyright (c) 2026 n24q02m.\n",
        )
        problems = cl.check(self.root)
        self.assertEqual(len(problems), 1)
        self.assertIn("differs from the canonical MIT text", problems[0])
        self.assertIn("NOTICE", problems[0])
        self.assertEqual(self.run_main(), 1)

    def test_other_copyright_holder_fails(self):
        self.clean_repo()
        self.write(
            "LICENSE",
            cl.CANONICAL_LICENSE.replace("pith-hash contributors", "someone else"),
        )
        self.assertEqual(len(cl.check(self.root)), 1)

    def test_trailing_whitespace_fails(self):
        self.clean_repo()
        self.write("LICENSE", cl.CANONICAL_LICENSE.replace("MIT License\n", "MIT License \n", 1))
        self.assertEqual(len(cl.check(self.root)), 1)

    def test_missing_notice_fails(self):
        self.clean_repo()
        (self.root / "NOTICE").unlink()
        problems = cl.check(self.root)
        self.assertEqual(len(problems), 1)
        self.assertIn("NOTICE is missing", problems[0])
        self.assertEqual(self.run_main(), 1)

    def test_empty_notice_fails(self):
        self.clean_repo()
        self.write("NOTICE", "  \n")
        problems = cl.check(self.root)
        self.assertEqual(len(problems), 1)
        self.assertIn("NOTICE is empty", problems[0])

    def test_both_problems_are_reported_together(self):
        self.write("LICENSE", "not a licence\n")
        self.assertEqual(len(cl.check(self.root)), 2)


class OwnRepositoryTests(unittest.TestCase):
    def test_this_repository_is_clean(self):
        root = pathlib.Path(__file__).resolve().parents[1]
        self.assertEqual(cl.check(root), [])


if __name__ == "__main__":
    unittest.main()
