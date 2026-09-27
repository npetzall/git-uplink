"""Line-ending coverage for the Cargo version rewrite."""

import unittest

import update_version_in_cargo as updater

VERSION = "0.0.98"
TOML = """\
[package]
name = "git-uplink"
version = "0.1.0"
edition = "2024"

[dependencies]
chrono = { version = "=0.4.45" }
"""
LOCK = """\
version = 4

[[package]]
name = "chrono"
version = "0.4.45"

[[package]]
name = "git-uplink"
version = "0.1.0"

[[package]]
name = "serde"
version = "1.0.229"
"""


class UpdateVersionTests(unittest.TestCase):
    def test_toml_and_lock_keep_lf(self) -> None:
        self._assert_rewrite("\n")

    def test_toml_and_lock_keep_crlf(self) -> None:
        self._assert_rewrite("\r\n")

    def _assert_rewrite(self, newline: str) -> None:
        toml = TOML.replace("\n", newline)
        lock = LOCK.replace("\n", newline)

        updated_toml = updater.update_cargo_toml(toml, VERSION)
        updated_lock = updater.update_cargo_lock(lock, VERSION)

        self.assertEqual(updated_toml.count(f'version = "{VERSION}"'), 1)
        self.assertIn(f'version = "{VERSION}"{newline}', updated_toml)
        self.assertIn('version = "=0.4.45"', updated_toml)
        self.assertNotIn('version = "0.1.0"', updated_toml)

        self.assertEqual(updated_lock.count(f'version = "{VERSION}"'), 1)
        self.assertIn(f'version = "{VERSION}"{newline}', updated_lock)
        self.assertIn(f'version = "0.4.45"{newline}', updated_lock)
        self.assertIn(f'version = "1.0.229"{newline}', updated_lock)
        self.assertNotIn('version = "0.1.0"', updated_lock)
        self.assertTrue(updated_toml.endswith(newline))
        self.assertTrue(updated_lock.endswith(newline))


if __name__ == "__main__":
    unittest.main()
