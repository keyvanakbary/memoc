import subprocess
import tempfile
import tomllib
import unittest
from pathlib import Path

from release import bump, prepare


class VersionRules(unittest.TestCase):
    def test_patch_for_fixes_and_every_other_commit(self):
        for message in ["fix: handle an empty folder", "perf(render): reduce allocations",
                        "docs: clarify usage", "chore: update CI", "Initial commit", "refactor: simplify"]:
            with self.subTest(message=message):
                self.assertEqual(bump("1.2.3", [message]), "1.2.4")

    def test_features_are_minor(self):
        self.assertEqual(bump("1.2.3", ["feat: add themes"]), "1.3.0")
        self.assertEqual(bump("0.1.9", ["feat(cli): add a command"]), "0.2.0")

    def test_breaking_changes_are_major_including_before_one(self):
        for message in ["feat!: change routes", "fix(cli)!: remove an option",
                        "refactor: simplify\n\nBREAKING CHANGE: URLs change",
                        "chore: update\n\nBREAKING-CHANGE: requires a new format"]:
            with self.subTest(message=message):
                self.assertEqual(bump("0.2.3", [message]), "1.0.0")
                self.assertEqual(bump("1.2.3", [message]), "2.0.0")

    def test_highest_bump_wins_once_for_the_whole_push(self):
        self.assertEqual(bump("1.2.3", ["fix: a", "feat: b", "fix: c"]), "1.3.0")
        self.assertEqual(bump("1.2.3", ["fix!: a", "feat: b", "docs: c"]), "2.0.0")

    def test_incidental_words_do_not_trigger_breaking_changes(self):
        self.assertEqual(bump("1.2.3", ["docs: explain BREAKING CHANGE: markers"]), "1.2.4")
        self.assertEqual(bump("1.2.3", ["fix: typo\n\nfeat: quoted example"]), "1.2.4")


class GitReleases(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="memoc-release-test-")
        self.root = Path(self.temporary.name)
        self.git("init", "--initial-branch=main")
        self.write_version("0.1.0")
        self.commit("chore: initial source")

    def tearDown(self):
        self.temporary.cleanup()

    def git(self, *args):
        return subprocess.check_output(["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", *args], cwd=self.root, text=True, stderr=subprocess.PIPE).strip()

    def write_version(self, version):
        (self.root / "Cargo.toml").write_text(f'[package]\nname = "memoc"\nversion = "{version}"\n\n[dependencies]\nother = "1"\n')
        (self.root / "Cargo.lock").write_text(f'version = 4\n\n[[package]]\nname = "memoc"\nversion = "{version}"\n\n[[package]]\nname = "other"\nversion = "0.1.0"\nsource = "registry+example"\n')

    def commit(self, message):
        self.git("add", "Cargo.toml", "Cargo.lock")
        self.git("commit", "--allow-empty", "-m", message)

    def test_initial_release_uses_cargo_as_baseline_and_updates_only_own_version(self):
        plan = prepare(self.root)
        self.assertEqual(plan, {"version": "0.1.1", "tag": "v0.1.1", "changed": "true"})
        self.assertEqual(tomllib.loads((self.root / "Cargo.toml").read_text())["package"]["version"], "0.1.1")
        packages = tomllib.loads((self.root / "Cargo.lock").read_text())["package"]
        self.assertEqual([p["version"] for p in packages], ["0.1.1", "0.1.0"])

    def test_only_commits_since_last_release_are_considered(self):
        self.commit("feat!: old breaking change")
        self.git("tag", "v0.1.0")
        self.commit("fix: current fix")
        self.assertEqual(prepare(self.root)["version"], "0.1.1")

    def test_release_rerun_resumes_without_bumping_again(self):
        plan = prepare(self.root)
        self.commit("chore(release): v0.1.1 [skip ci]")
        self.git("tag", plan["tag"])
        self.assertEqual(prepare(self.root), {**plan, "changed": "false"})

    def test_next_push_advances_after_the_release_commit(self):
        prepare(self.root)
        self.commit("chore(release): v0.1.1 [skip ci]")
        self.git("tag", "v0.1.1")
        self.commit("fix: first fix")
        self.commit("feat: feature")
        self.assertEqual(prepare(self.root)["version"], "0.2.0")

    def test_manual_version_changes_and_lock_mismatches_are_rejected(self):
        self.git("tag", "v0.1.0")
        self.write_version("0.3.0")
        self.commit("chore: manually bumped")
        with self.assertRaisesRegex(ValueError, "automatically"):
            prepare(self.root)
        (self.root / "Cargo.lock").write_text('[[package]]\nname = "memoc"\nversion = "0.1.0"\n')
        with self.assertRaisesRegex(ValueError, "must match"):
            prepare(self.root)

    def test_latest_version_uses_numeric_order_and_ignores_prereleases(self):
        self.git("tag", "v0.1.0")
        self.write_version("0.10.0")
        self.commit("chore(release): v0.10.0")
        self.git("tag", "v0.10.0")
        self.git("tag", "v1.0.0-rc.1")
        self.commit("fix: next")
        self.assertEqual(prepare(self.root)["version"], "0.10.1")


if __name__ == "__main__":
    unittest.main()
