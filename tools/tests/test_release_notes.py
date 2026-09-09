"""Exercise release-note boundaries against real, disposable Git histories."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


def publication(tag, *, draft=False, published_at="2026-01-01T00:00:00Z"):
    return dict(tag_name=tag, draft=draft, published_at=published_at)


class ReleaseNotesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.env = os.environ.copy()
        # Hooks export Git variables. Never allow fixture commits in the caller.
        for name in subprocess.check_output(["git", "rev-parse", "--local-env-vars"], text=True).splitlines():
            self.env.pop(name, None)
        self.env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
        scripts = self.repo / "tools/os/scripts"
        scripts.mkdir(parents=True)
        for name in ("generate-release-notes.py", "generate-stable-release-notes.sh"):
            shutil.copy2(ROOT / "tools/os/scripts" / name, scripts / name)
        self.generator = scripts / "generate-release-notes.py"
        self.output = self.repo / "notes.md"
        self.catalog = self.repo / "publications.json"
        self.bin = self.repo / "bin"
        self.bin.mkdir()
        gh = self.bin / "gh"
        gh.write_text('''#!/usr/bin/env python3
import os, pathlib, sys
pathlib.Path(os.environ["GH_ARGS"]).write_text("\\n".join(sys.argv[1:]))
if os.environ.get("GH_FAIL"):
    sys.exit(1)
print(os.environ["GH_PAGES"])
''')
        gh.chmod(0o755)
        self.env.update(PATH=f"{self.bin}{os.pathsep}{self.env['PATH']}",
                        GH_ARGS=str(self.repo / "gh-args"), GH_PAGES="[]")
        self.git("init", "-q")
        self.git("config", "user.name", "Release Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "tag.gpgsign", "false")
        self.commit("Initial stable source", "v0.6.100-stable")
        self.git("tag", "v0.6.99-beta")
        self.commit("Improve discovery", "v0.6.101-beta")
        self.commit("Release v0.6.102-beta", "v0.6.102-beta")
        self.commit("Fix reconnect", "v0.6.103-beta")
        self.git("tag", "-a", "v0.6.103-stable", "-m", "Promote exact beta")
        self.commit("Improve scheduling", "v0.6.104-beta")

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args],
                                       env=self.env, text=True, stderr=subprocess.STDOUT).strip()

    def commit(self, subject, tag=None):
        self.git("commit", "--allow-empty", "-qm", subject)
        if tag:
            self.git("tag", tag)

    def run_notes(self, tag, releases=None, *options, stable_wrapper=False, success=True):
        command = (["bash", str(self.generator.with_name("generate-stable-release-notes.sh"))]
                   if stable_wrapper else [sys.executable, str(self.generator)])
        command += [tag, str(self.output), "--repository", "example/product", *options]
        if releases is not None:
            self.catalog.write_text(json.dumps(releases))
            command += ["--published-releases", str(self.catalog)]
        result = subprocess.run(command, cwd=self.repo, env=self.env, text=True, capture_output=True)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
            return self.output.read_text()
        self.assertNotEqual(result.returncode, 0)
        return result.stderr

    def test_beta_skips_unpublished_and_draft_tags_and_version_bumps(self):
        body = self.run_notes("v0.6.103-beta", [
            publication("v0.6.99-beta"), publication("v0.6.100-stable"),
            publication("v0.6.101-beta"), publication("v0.6.102-beta", draft=True)])
        self.assertIn("Changes since v0.6.101-beta", body)
        self.assertIn("- Fix reconnect", body)
        self.assertNotIn("Improve discovery", body)
        self.assertNotIn("- Release v", body)
        self.assertNotIn("Improve scheduling", body)  # exact tag, not checkout HEAD
        self.assertIn("/compare/v0.6.101-beta...v0.6.103-beta", body)
        self.assertIn(self.git("rev-parse", "v0.6.103-beta"), body)

    def test_stable_is_cumulative_since_previous_stable(self):
        body = self.run_notes("v0.6.103-stable", [
            publication("v0.6.100-stable"), publication("v0.6.101-beta"),
            publication("v0.6.102-beta"), publication("v0.6.103-beta")], stable_wrapper=True)
        self.assertIn("Changes since v0.6.100-stable", body)
        self.assertIn("Improve discovery", body)
        self.assertIn("Fix reconnect", body)
        self.assertNotIn("- Release v", body)

    def test_first_beta_after_stable_uses_stable(self):
        body = self.run_notes("v0.6.104-beta", [
            publication("v0.6.101-beta"), publication("v0.6.103-beta"),
            publication("v0.6.103-stable")])
        self.assertIn("Changes since v0.6.103-stable", body)
        self.assertIn("Improve scheduling", body)
        self.assertNotIn("Fix reconnect", body)

    def test_first_beta_falls_back_to_stable_and_first_stable_has_full_history(self):
        body = self.run_notes("v0.6.101-beta", [publication("v0.6.100-stable")])
        self.assertIn("Changes since v0.6.100-stable", body)
        body = self.run_notes("v0.6.103-stable", [publication("v0.6.103-beta")])
        self.assertIn("first published stable release", body)
        self.assertIn("Initial stable source", body)
        self.assertIn("Improve discovery", body)
        self.assertNotIn("/compare/", body)

    def test_same_commit_baseline_produces_explicit_empty_notes(self):
        self.git("tag", "v0.6.105-beta", "v0.6.104-beta")
        body = self.run_notes("v0.6.105-beta", [publication("v0.6.104-beta")])
        self.assertIn("Changes since v0.6.104-beta", body)
        self.assertIn("No additional source changes.", body)

    def test_ignores_other_branch_and_future_versions(self):
        self.git("checkout", "-qb", "other", "v0.6.100-stable")
        self.commit("Other branch", "v0.6.102-stable")
        body = self.run_notes("v0.6.103-beta", [
            publication("v0.6.101-beta"), publication("v0.6.102-stable"),
            publication("v0.6.104-beta")])
        self.assertIn("Changes since v0.6.101-beta", body)
        self.assertNotIn("Other branch", body)

    def test_regeneration_skips_releases_published_after_current_release(self):
        body = self.run_notes("v0.6.103-beta", [
            publication("v0.6.101-beta"),
            publication("v0.6.102-beta", published_at="2026-03-01T00:00:00Z"),
            publication("v0.6.103-beta", published_at="2026-02-01T00:00:00Z")])
        self.assertIn("Changes since v0.6.101-beta", body)

    def test_paginates_public_api_and_combines_legacy_metadata_without_private_links(self):
        self.env["GH_PAGES"] = json.dumps([[publication("v0.6.104-beta")],
                                           [publication("v0.6.101-beta")]])
        legacy = self.repo / "legacy.json"
        legacy.write_text(json.dumps([dict(publication("v0.6.100-stable"),
                                          body="PRIVATE BODY", html_url="https://private.invalid")]))
        body = self.run_notes("v0.6.103-beta", None, "--additional-publications", str(legacy))
        self.assertIn("Changes since v0.6.101-beta", body)
        args = (self.repo / "gh-args").read_text()
        self.assertIn("--paginate\n--slurp", args)
        self.assertIn("repos/example/product/releases?per_page=100", args)
        self.env["GH_PAGES"] = "[]"
        body = self.run_notes("v0.6.103-beta", None, "--additional-publications", str(legacy))
        self.assertIn("Changes since v0.6.100-stable", body)
        self.assertNotIn("PRIVATE", body)
        self.assertNotIn("private.invalid", body)
        self.assertIn("https://github.com/example/product/compare/", body)

    def test_api_failure_leaves_existing_output_untouched(self):
        self.env["GH_FAIL"] = "1"
        self.output.write_text("Existing reviewed notes")
        self.run_notes("v0.6.103-beta", success=False)
        self.assertEqual(self.output.read_text(), "Existing reviewed notes")

    def test_missing_published_tag_fails_instead_of_silently_using_older_baseline(self):
        error = self.run_notes("v0.6.103-beta", [publication("v0.6.102-stable")], success=False)
        self.assertIn("fetch full history and tags", error)
        self.assertFalse(self.output.exists())

    def test_unpublished_metadata_is_not_a_baseline(self):
        body = self.run_notes("v0.6.103-beta", [
            publication("v0.6.101-beta"), publication("v0.6.102-beta", published_at=None)])
        self.assertIn("Changes since v0.6.101-beta", body)


if __name__ == "__main__":
    unittest.main()
