#!/usr/bin/env python3

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parent


class ZednotesToolingTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="zednotes-tooling-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.upstream = self.root / "upstream"
        self.repository = self.root / "fork"
        self.environment = {
            key: value for key, value in os.environ.items()
            if not key.startswith(("GIT_", "ZEDNOTES_"))
        }
        self.environment.update({
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_AUTHOR_NAME": "Tooling Test",
            "GIT_AUTHOR_EMAIL": "test@example.com",
            "GIT_COMMITTER_NAME": "Tooling Test",
            "GIT_COMMITTER_EMAIL": "test@example.com",
            "GIT_TERMINAL_PROMPT": "0",
            "GIT_EDITOR": "true",
        })
        self.run_command("git", "init", "--quiet", "--initial-branch=main", str(self.upstream), directory=self.root)
        manifest = self.upstream / "crates/zed/Cargo.toml"
        manifest.parent.mkdir(parents=True)
        manifest.write_text('[package]\nversion = "1.0.0"\n')
        for name in ["shared", "local"]:
            (self.upstream / name).write_text("initial\n")
        self.git("add", ".", directory=self.upstream)
        self.git("commit", "--quiet", "-m", "Initial", directory=self.upstream)
        self.base = self.git("rev-parse", "HEAD", directory=self.upstream)
        self.run_command("git", "clone", "--quiet", str(self.upstream), str(self.repository), directory=self.root)
        self.git("remote", "rename", "origin", "upstream")
        self.git("checkout", "--quiet", "-b", "dev")
        (self.repository / "script").mkdir()
        for name in ["merge-upstream", "zednotes-version"]:
            shutil.copy2(SCRIPTS / name, self.repository / "script" / name)
        (self.repository / "zednotes.toml").write_text(
            '[zednotes]\nversion = "0.5.0"\n\n[upstream]\n'
            f'zed_version = "1.0.0"\ncommit = "{self.base}"\n'
        )
        self.git("add", ".")
        self.git("commit", "--quiet", "-m", "Fork tooling")

    def run_command(self, *arguments, directory=None, check=True):
        return subprocess.run(
            arguments, cwd=directory or self.repository, env=self.environment,
            text=True, capture_output=True, check=check, timeout=30,
        )

    def git(self, *arguments, directory=None):
        return self.run_command("git", *arguments, directory=directory).stdout.strip()

    def script(self, name, *arguments):
        return self.run_command("bash", f"script/{name}", *arguments, check=False)

    def advance_upstream(self, conflict=False):
        (self.upstream / "crates/zed/Cargo.toml").write_text('[package]\nversion = "1.1.0"\n')
        if conflict:
            (self.upstream / "shared").write_text("upstream\n")
        self.git("add", ".", directory=self.upstream)
        self.git("commit", "--quiet", "-m", "Advance upstream", directory=self.upstream)
        return self.git("rev-parse", "HEAD", directory=self.upstream)

    def dirty_tree(self):
        (self.repository / "local").write_text("staged\n")
        self.git("add", "local")
        (self.repository / "local").write_text("unstaged\n")
        (self.repository / "untracked").write_text("untracked\n")

    def snapshot(self):
        return (
            self.git("rev-parse", "dev", "main", "HEAD"),
            self.git("symbolic-ref", "--short", "HEAD"),
            self.git("status", "--porcelain"),
            self.git("diff"), self.git("diff", "--cached"),
            (self.repository / "untracked").read_text(),
        )

    def make_conflict(self):
        (self.repository / "shared").write_text("fork\n")
        self.git("add", "shared")
        self.git("commit", "--quiet", "-m", "Fork change")
        self.advance_upstream(conflict=True)

    def test_success_restores_original_branch_and_index(self):
        self.git("checkout", "--quiet", "-b", "topic")
        self.dirty_tree()
        before = self.snapshot()
        upstream = self.advance_upstream()
        result = self.script("merge-upstream", "--stash")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("branch", "--show-current"), "topic")
        self.assertEqual(self.snapshot()[2:], before[2:])
        self.assertEqual(self.git("rev-parse", "main"), upstream)
        self.assertEqual(self.git("stash", "list"), "")
        self.assertIn(upstream, self.git("show", "dev:zednotes.toml"))

    def test_fetch_failure_restores_everything(self):
        self.git("checkout", "--quiet", "-b", "topic")
        self.dirty_tree()
        before = self.snapshot()
        self.git("remote", "set-url", "upstream", str(self.root / "missing"))
        result = self.script("merge-upstream", "--stash")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(self.git("stash", "list"), "")

    def test_integration_ahead_of_upstream_is_rejected(self):
        self.git("checkout", "--quiet", "main")
        (self.repository / "local").write_text("integration-only commit\n")
        self.git("add", "local")
        self.git("commit", "--quiet", "-m", "Integration change")
        self.git("checkout", "--quiet", "dev")
        self.dirty_tree()
        before = self.snapshot()
        result = self.script("merge-upstream", "--stash")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.snapshot(), before)

    def test_pin_parse_failure_rolls_back(self):
        (self.repository / "zednotes.toml").write_text("invalid TOML [\n")
        self.git("add", "zednotes.toml")
        self.git("commit", "--quiet", "-m", "Invalid pin")
        self.advance_upstream()
        self.dirty_tree()
        before = self.snapshot()
        result = self.script("merge-upstream", "--stash")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.snapshot(), before)

    def test_conflict_rolls_back(self):
        self.make_conflict()
        self.dirty_tree()
        before = self.snapshot()
        result = self.script("merge-upstream", "--stash")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.snapshot(), before)
        self.assertFalse((self.repository / ".git/MERGE_HEAD").exists())

    def test_pin_commit_failure_rolls_back(self):
        self.advance_upstream()
        hook = self.repository / ".git/hooks/pre-commit"
        hook.write_text("#!/bin/sh\nexit 1\n")
        hook.chmod(0o755)
        self.dirty_tree()
        before = self.snapshot()
        result = self.script("merge-upstream", "--stash")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.snapshot(), before)

    def test_failed_rollback_checkout_never_resets_current_branch(self):
        upstream = self.advance_upstream()
        original_fork = self.git("rev-parse", "dev")
        self.dirty_tree()
        binaries = self.root / "bin"
        binaries.mkdir()
        wrapper = binaries / "git"
        wrapper.write_text(
            '#!/bin/bash\n'
            'if [[ "$1" == checkout && ( "${3:-}" == dev || " $* " == *" --detach "* ) ]]; then\n'
            '    exit 1\n'
            'fi\n'
            'exec "$REAL_GIT" "$@"\n'
        )
        wrapper.chmod(0o755)
        self.environment["REAL_GIT"] = shutil.which("git")
        self.environment["PATH"] = f'{binaries}{os.pathsep}{self.environment["PATH"]}'
        result = self.script("merge-upstream", "--stash")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("rollback incomplete", result.stderr)
        self.assertEqual(self.git("rev-parse", "main"), upstream)
        self.assertEqual(self.git("rev-parse", "dev"), original_fork)
        self.assertNotEqual(self.git("stash", "list"), "")

    def test_leave_conflicts_retains_autostash(self):
        self.make_conflict()
        self.dirty_tree()
        result = self.script("merge-upstream", "--stash", "--leave-conflicts")
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue((self.repository / ".git/MERGE_HEAD").exists())
        self.assertIn("autostash retained", result.stderr)
        self.assertNotEqual(self.git("stash", "list"), "")

    def test_stash_conflict_does_not_undo_completed_merge(self):
        upstream = self.advance_upstream(conflict=True)
        (self.repository / "shared").write_text("local work\n")
        result = self.script("merge-upstream", "--stash")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.git("rev-parse", "main"), upstream)
        self.assertIn(upstream, self.git("show", "dev:zednotes.toml"))
        self.assertNotEqual(self.git("stash", "list"), "")
        self.assertIn("local work", (self.repository / "shared").read_text())

    def test_existing_stash_is_preserved(self):
        (self.repository / "local").write_text("old stash\n")
        self.git("stash", "push", "--quiet")
        old_stash = self.git("rev-parse", "refs/stash")
        self.dirty_tree()
        self.advance_upstream()
        result = self.script("merge-upstream", "--stash")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", "refs/stash"), old_stash)
        self.assertEqual((self.repository / "local").read_text(), "unstaged\n")

    def test_detached_checkout_is_restored(self):
        original = self.git("rev-parse", "HEAD")
        self.git("checkout", "--quiet", "--detach")
        self.dirty_tree()
        self.advance_upstream()
        result = self.script("merge-upstream", "--stash")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), original)
        self.assertEqual(self.git("branch", "--show-current"), "")
        self.assertEqual((self.repository / "local").read_text(), "unstaged\n")

    def test_no_commit(self):
        upstream = self.advance_upstream()
        result = self.script("merge-upstream", "--no-commit")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(upstream, (self.repository / "zednotes.toml").read_text())
        self.assertEqual(self.git("status", "--porcelain"), "M zednotes.toml")
        self.assertEqual(self.git("diff", "--cached"), "")

    def test_no_commit_rejects_other_starting_branch(self):
        self.git("checkout", "--quiet", "-b", "topic")
        result = self.script("merge-upstream", "--no-commit")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.git("branch", "--show-current"), "topic")

    def test_version_parser_failure_propagates(self):
        for contents in ['invalid [', '[zednotes]\nversion = 5\n', '[zednotes]\nversion = ""\n[upstream]\nzed_version = "1"\ncommit = "abc"\n']:
            with self.subTest(contents=contents):
                (self.repository / "zednotes.toml").write_text(contents)
                result = self.script("zednotes-version", "show")
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")

    def test_stale_pin_is_rejected(self):
        upstream = self.advance_upstream()
        self.git("fetch", "--quiet", "upstream")
        self.git("merge", "--quiet", "--no-edit", upstream)
        result = self.script("zednotes-version", "check")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("is stale", result.stderr)

    def test_repeated_merge_does_not_add_commits(self):
        self.advance_upstream()
        result = self.script("merge-upstream")
        self.assertEqual(result.returncode, 0, result.stderr)
        head = self.git("rev-parse", "HEAD")
        result = self.script("merge-upstream")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.git("rev-parse", "HEAD"), head)

    def test_version_check_and_custom_upstream_branch(self):
        self.assertEqual(self.script("zednotes-version", "check").returncode, 0)
        self.git("branch", "-m", "main", "next", directory=self.upstream)
        self.environment["ZEDNOTES_UPSTREAM_BRANCH"] = "next"
        self.advance_upstream()
        self.git("fetch", "--quiet", "upstream")
        result = self.script("merge-upstream")
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.script("zednotes-version", "check")
        self.assertEqual(result.returncode, 0, result.stderr)
        (self.repository / "zednotes.toml").write_text(
            (self.repository / "zednotes.toml").read_text().replace('"1.1.0"', '"9.0.0"')
        )
        self.assertNotEqual(self.script("zednotes-version", "check").returncode, 0)


if __name__ == "__main__":
    unittest.main()
