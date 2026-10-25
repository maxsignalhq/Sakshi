"""Tests for render.py. Run with: python3 -m unittest discover -s packaging"""

import json
import re
import unittest

import render


def fake_sums(version="1.2.3", skip=None):
    lines = []
    for index, target in enumerate(render.TARGETS):
        if target == skip:
            continue
        digest = f"{index + 1:x}" * 64
        lines.append(f"{digest}  {render.archive_name(version, target)}")
    return "\n".join(lines) + "\n"


class RenderTests(unittest.TestCase):
    def test_every_template_renders_with_no_placeholder_left(self):
        out = render.render("1.2.3", fake_sums())
        self.assertEqual(
            sorted(out),
            [
                "homebrew/sakshi.rb",
                "scoop/sakshi.json",
                "winget/MaxSignalHQ.Sakshi.installer.yaml",
                "winget/MaxSignalHQ.Sakshi.locale.en-US.yaml",
                "winget/MaxSignalHQ.Sakshi.yaml",
            ],
        )
        for path, text in out.items():
            self.assertNotRegex(text, r"@@[A-Z0-9_]+@@", path)
            self.assertIn("1.2.3", text, path)

    def test_a_v_prefix_on_the_version_is_ignored(self):
        out = render.render("v1.2.3", fake_sums())
        self.assertNotIn("vv1.2.3", out["homebrew/sakshi.rb"])
        self.assertIn('version "1.2.3"', out["homebrew/sakshi.rb"])

    def test_scoop_manifest_is_valid_json_with_the_windows_checksum(self):
        out = render.render("1.2.3", fake_sums())
        scoop = json.loads(out["scoop/sakshi.json"])
        arch = scoop["architecture"]["64bit"]
        self.assertTrue(arch["url"].endswith("sakshi-v1.2.3-x86_64-pc-windows-msvc.zip"))
        self.assertEqual(arch["hash"], "5" * 64)
        self.assertEqual(scoop["version"], "1.2.3")
        arm = scoop["architecture"]["arm64"]
        self.assertTrue(arm["url"].endswith("sakshi-v1.2.3-aarch64-pc-windows-msvc.zip"))
        self.assertEqual(arm["hash"], "6" * 64)

    def test_homebrew_gets_each_platforms_own_checksum(self):
        formula = render.render("1.2.3", fake_sums())["homebrew/sakshi.rb"]
        pairs = re.findall(r'url ".*?/(sakshi-[^"]+)"\s+sha256 "([0-9a-f]+)"', formula)
        self.assertEqual(len(pairs), 4)
        expected = {
            render.archive_name("1.2.3", t): f"{i + 1:x}" * 64
            for i, t in enumerate(render.TARGETS)
        }
        for name, digest in pairs:
            self.assertEqual(expected[name], digest, name)

    def test_winget_installer_points_at_the_zip_and_its_checksum(self):
        text = render.render("1.2.3", fake_sums())["winget/MaxSignalHQ.Sakshi.installer.yaml"]
        self.assertIn("sakshi-v1.2.3-x86_64-pc-windows-msvc.zip", text)
        self.assertIn("InstallerSha256: " + "5" * 64, text)
        self.assertIn("PortableCommandAlias: sakshi", text)
        self.assertIn("sakshi-v1.2.3-aarch64-pc-windows-msvc.zip", text)
        self.assertIn("InstallerSha256: " + "6" * 64, text)

    def test_a_missing_checksum_is_an_error(self):
        with self.assertRaisesRegex(ValueError, "no checksum for"):
            render.render("1.2.3", fake_sums(skip="aarch64-apple-darwin"))

    def test_checksums_for_another_version_do_not_count(self):
        with self.assertRaisesRegex(ValueError, "no checksum for"):
            render.render("9.9.9", fake_sums("1.2.3"))

    def test_unreadable_checksum_lines_are_rejected(self):
        for bad in ["nothex  file.tar.gz", "abc", "z" * 64 + "  f"]:
            with self.assertRaises(ValueError, msg=bad):
                render.parse_sums(bad)

    def test_parse_sums_handles_blank_lines_binary_marker_and_case(self):
        digest = "A" * 64
        sums = render.parse_sums(f"\n{digest} *file.zip\n\n")
        self.assertEqual(sums, {"file.zip": "a" * 64})

    def test_an_unknown_placeholder_is_an_error(self):
        with self.assertRaisesRegex(ValueError, "unknown placeholder"):
            render.render_text("x @@NOPE@@", {"VERSION": "1"}, "t")


if __name__ == "__main__":
    unittest.main()
