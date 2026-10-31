"""Check update metadata without building or running an AppImage."""

import unittest

import build


class UpdateMetadataTest(unittest.TestCase):
    def test_fork_release_metadata_for_stable_and_nightly(self):
        for version, channel in (("1.2.3", "latest"), ("1.2.3-nightly.1", "latest-pre")):
            name, information, url = build.update_metadata(version, f"v{version}")
            self.assertEqual(name, f"serein-v{version}-Linux-X64.AppImage")
            self.assertEqual(
                information,
                f"gh-releases-zsync|MCShotty|Serein|{channel}|serein-*-Linux-X64.AppImage.zsync",
            )
            self.assertEqual(
                url,
                f"https://github.com/MCShotty/Serein/releases/download/v{version}/{name}",
            )

    def test_invalid_release_tag_is_rejected(self):
        with self.assertRaises(ValueError):
            build.update_metadata("1.2.3", "v1.2.4")


if __name__ == "__main__":
    unittest.main()
