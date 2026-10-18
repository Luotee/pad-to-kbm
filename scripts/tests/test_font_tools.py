"""Unit tests for scripts/font_tools.py (run: python3 -m unittest scripts/tests/test_font_tools.py)."""
import hashlib, os, pathlib, sys, tempfile, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))


class FontToolsTest(unittest.TestCase):
    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp())
        self._old = os.environ.get("XDG_CACHE_HOME")
        os.environ["XDG_CACHE_HOME"] = str(self.tmp / "cache")
        self.src = self.tmp / "src.ttf"
        self.src.write_bytes(b"pretend font bytes")
        self.url = self.src.as_uri()
        self.sha = hashlib.sha256(self.src.read_bytes()).hexdigest()
        import font_tools
        self.ft = font_tools

    def tearDown(self):
        if self._old is None:
            os.environ.pop("XDG_CACHE_HOME", None)
        else:
            os.environ["XDG_CACHE_HOME"] = self._old

    def test_tools_dir_honours_xdg(self):
        self.assertEqual(self.ft.tools_dir(), self.tmp / "cache" / "pad-to-kbm-tools")

    def test_font_ok(self):
        p = self.ft.ensure_font(self.url, self.sha)
        self.assertEqual(p.read_bytes(), self.src.read_bytes())

    def test_font_mismatch_leaves_nothing(self):
        with self.assertRaises(RuntimeError):
            self.ft.ensure_font(self.url, "0" * 64)
        fonts = self.ft.tools_dir() / "fonts"
        self.assertEqual(list(fonts.glob("*")) if fonts.exists() else [], [])

    def test_font_corrupt_cache_refetched(self):
        target = self.ft.font_path()
        target.parent.mkdir(parents=True)
        target.write_bytes(b"junk")
        p = self.ft.ensure_font(self.url, self.sha)
        self.assertEqual(p.read_bytes(), self.src.read_bytes())

    def test_font_custom_target(self):
        p = self.ft.ensure_font(self.url, self.sha, self.ft.silk_path())
        self.assertEqual(p, self.ft.silk_path())
        self.assertFalse(self.ft.font_path().exists())


if __name__ == "__main__":
    unittest.main()
