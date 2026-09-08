"""Offline Skill integration checks: mocked GitHub and isolated real Git repositories."""

import argparse
import contextlib
import datetime as dt
import importlib.util
import io
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "skills/issue-finder/scripts/issue_finder.py"
SPEC = importlib.util.spec_from_file_location("issue_finder_skill", SCRIPT)
skill = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(skill)


def timestamp(days=0):
    return (dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=days)).isoformat()


def issue(number=1, **updates):
    return {"number": number, "title": "Fix parser error", "state": "open",
            "body": "Steps to reproduce: parse an empty string. Expected: a useful validation error.",
            "html_url": f"https://github.com/example/project/issues/{number}",
            "repository_url": "https://api.github.com/repos/example/project",
            "labels": [{"name": "bug"}], "updated_at": timestamp(), **updates}


def repository(**updates):
    return {"archived": False, "default_branch": "main", "pushed_at": timestamp(), **updates}


def check_command(code):
    return shlex.join([sys.executable, "-c", code])


@contextlib.contextmanager
def in_directory(path):
    previous = Path.cwd()
    os.chdir(path)
    try:
        yield
    finally:
        os.chdir(previous)


class DiscoveryTests(unittest.TestCase):
    def test_offline_recommendation_cases(self):
        data = json.loads((ROOT / "tests/fixtures/recommendation_eval/skill_native.json").read_text())
        for sample in data["samples"]:
            with self.subTest(sample=sample["id"]):
                record = issue(**sample["issue"])
                record["labels"] = [{"name": label} for label in sample["issue"].get("labels", [])]
                comments = [{**c, "updated_at": timestamp(c["updatedAgeDays"])} for c in sample.get("comments", [])]
                result = skill.assess(record, repository(**sample.get("repository", {})),
                                      comments, sample.get("timeline", []))
                self.assertEqual(bool(result["rejectedReasons"]), sample["expected"]["behavior"] == "hidden")

    def test_scout_excludes_unverified_competition_and_keeps_output_bounded(self):
        def fake_api(endpoint, fields=None, paginate=False):
            if endpoint == "repos/example/project":
                return repository()
            if endpoint == "search/issues":
                return {"items": [issue(1), issue(2), issue(3)], "total_count": 3}
            if endpoint.endswith("/timeline") and "/2/" in endpoint:
                raise skill.WorkflowError("rate limit")
            if endpoint.endswith(("/comments", "/timeline")):
                return []
            return issue(int(endpoint.rsplit("/", 1)[1]))

        with patch.object(skill, "api", side_effect=fake_api):
            result = skill.scout(argparse.Namespace(repo="example/project", query="parser", limit=1, scan_limit=8))
        self.assertTrue(result["ok"])
        self.assertEqual(len(result["candidates"]), 1)
        self.assertEqual(result["candidates"][0]["number"], 1)
        self.assertIn("#2 excluded", result["warnings"][0])

    def test_api_reads_competition_from_all_pages(self):
        pages = [[{"source": {"issue": {"state": "closed", "pull_request": {"url": "closed"}}}}],
                 [{"source": {"issue": {"state": "open", "pull_request": {"url": "open"}}}}]]
        with patch.object(skill, "run", return_value=subprocess.CompletedProcess([], 0, json.dumps(pages), "")) as call:
            events = skill.api("repos/example/project/issues/1/timeline", {"per_page": 100}, paginate=True)
        self.assertIn("--paginate", call.call_args.args[0])
        self.assertIn("--slurp", call.call_args.args[0])
        self.assertEqual(call.call_args.args[0][call.call_args.args[0].index("--method") + 1], "GET")
        self.assertIn("open pull request references this issue", skill.assess(issue(), repository(), [], events)["rejectedReasons"])

    def test_prepare_rechecks_candidate_before_touching_git(self):
        with patch.object(skill, "api", return_value=repository()), \
                patch.object(skill, "issue_evidence", return_value=(issue(state="closed"), [], [])), \
                patch.object(skill, "prepare_workspace") as prepare:
            result = skill.prepare(argparse.Namespace(issue="example/project#1", workspace_root="unused"))
        self.assertEqual(result["status"], "candidate_unavailable")
        prepare.assert_not_called()

    def test_repo_and_issue_inputs_cannot_be_paths_or_options(self):
        for value in ("--help", "../outside", "example/..", "example/repo;touch /tmp/x"):
            with self.subTest(value=value), self.assertRaises(skill.WorkflowError):
                skill.repo_name(value)
        for url in ("https://example.org/owner/repo/issues/1", "example/project#0"):
            with self.assertRaises(skill.WorkflowError):
                skill.parse_issue(url)
        self.assertEqual(skill.parse_issue("https://github.com/example/project/issues/12"), ("example/project", 12))
        self.assertEqual(skill.remote_repo("git@github.com:example/project.git"), "example/project")

    def test_cli_errors_are_single_json_with_nonzero_exit(self):
        for argv in (["scout", "--limit", "0", "--json"], ["finish", "--workspace", ".", "--json"]):
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                code = skill.main(argv)
            self.assertEqual(code, 1)
            self.assertFalse(json.loads(output.getvalue())["ok"])


@unittest.skipUnless(shutil.which("git"), "git is required for local workspace integration")
class WorkspaceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.source = self.root / "source"
        self.source.mkdir()
        # Ignore user Git identity, hooks, credentials, signing, and URL rewrites.
        self.environment = patch.dict(os.environ, {
            "GIT_CONFIG_GLOBAL": str(self.root / "gitconfig"), "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_AUTHOR_NAME": "Skill Test", "GIT_AUTHOR_EMAIL": "test@example.invalid",
            "GIT_COMMITTER_NAME": "Skill Test", "GIT_COMMITTER_EMAIL": "test@example.invalid",
        })
        self.environment.start()
        self.addCleanup(self.environment.stop)
        skill.git(self.source, "init", "-b", "main")
        (self.source / "source.txt").write_text("original\n")
        (self.source / "AGENTS.md").write_text("Use focused tests.\n")
        (self.source / "Cargo.toml").write_text('[package]\nname = "example"\n')
        skill.git(self.source, "add", ".")
        skill.git(self.source, "commit", "-m", "initial")
        skill.git(self.source, "remote", "add", "origin", "https://github.com/example/project.git")
        # Exercise real clone/fetch/worktree with local transport only.
        skill.run(["git", "config", "--global", f"url.{self.source}.insteadOf", "https://github.com/example/project.git"])
        self.contributions = self.root / "contributions"

    def prepare(self, cwd=None):
        with in_directory(cwd or self.source), \
                patch.object(skill, "api", return_value=repository()), \
                patch.object(skill, "issue_evidence", return_value=(issue(), [], [])):
            return skill.prepare(argparse.Namespace(issue="example/project#1", workspace_root=str(self.contributions)))

    def finish(self, checks=None, workspace=None, timeout=30):
        return skill.finish(argparse.Namespace(workspace=str(workspace or self.source),
                                               check=checks or [check_command("assert open('source.txt').read() == 'fixed\\n'")],
                                               check_timeout=timeout))

    def test_clean_checkout_branches_and_creates_only_one_task_artifact(self):
        base = skill.git(self.source, "rev-parse", "HEAD")
        result = self.prepare()
        self.assertEqual(Path(result["workspace"]["path"]), self.source)
        self.assertEqual(result["workspace"]["baseCommit"], base)
        self.assertTrue(result["workspace"]["branch"].startswith("fuyue/issue-1-"))
        self.assertEqual(skill.git(self.source, "status", "--porcelain"), "?? .issue-finder-task.json")
        self.assertIn("AGENTS.md", result["repo"]["instructionFiles"])
        self.assertIn("cargo test", result["repo"]["suggestedChecks"])

    def test_dirty_checkout_is_isolated_without_stashing_or_losing_edits(self):
        (self.source / "source.txt").write_text("user's work\n")
        skill.git(self.source, "add", "source.txt")
        (self.source / "unrelated.txt").write_text("keep me")
        before = skill.git(self.source, "status", "--porcelain")
        result = self.prepare()
        workspace = Path(result["workspace"]["path"])
        self.assertNotEqual(workspace, self.source)
        self.assertEqual((workspace / "source.txt").read_text(), "original\n")
        self.assertEqual((self.source / "source.txt").read_text(), "user's work\n")
        self.assertEqual(skill.git(self.source, "status", "--porcelain"), before)
        self.assertEqual(skill.git(self.source, "branch", "--show-current"), "main")

    def test_clone_then_reuse_cache_without_overwriting_pending_work(self):
        other = self.root / "other"
        other.mkdir()
        first = self.prepare(other)
        cached = Path(first["workspace"]["path"])
        (cached / "source.txt").write_text("pending work")
        second = self.prepare(other)
        self.assertNotEqual(first["workspace"]["path"], second["workspace"]["path"])
        self.assertEqual((cached / "source.txt").read_text(), "pending work")
        self.assertTrue((cached / skill.TASK_FILE).exists())

    def test_nested_contribution_root_is_rejected_without_creating_it(self):
        with self.assertRaises(skill.WorkflowError):
            skill.contribution_root(str(self.source / "nested"))
        self.assertFalse((self.source / "nested").exists())

    def test_symlinked_cache_parent_cannot_escape_contribution_root(self):
        self.contributions.mkdir()
        (self.contributions / "example").symlink_to(self.root, target_is_directory=True)
        with self.assertRaises(skill.WorkflowError):
            self.prepare(self.root)
        self.assertFalse((self.root / "project").exists())

    def test_existing_task_is_preserved_for_resume(self):
        first = self.prepare()
        contents = (self.source / skill.TASK_FILE).read_bytes()
        with self.assertRaises(skill.WorkflowError):
            self.prepare()
        self.assertEqual((self.source / skill.TASK_FILE).read_bytes(), contents)
        self.assertEqual(skill.git(self.source, "branch", "--show-current"), first["workspace"]["branch"])

    def test_failed_validation_keeps_task_then_retry_reports_all_changes(self):
        self.prepare()
        failure = self.finish()
        self.assertEqual(failure["status"], "validation_failed")
        self.assertIn("AssertionError", failure["checks"][0]["outputTail"])
        self.assertTrue((self.source / skill.TASK_FILE).exists())
        (self.source / "source.txt").write_text("fixed\n")
        skill.git(self.source, "add", "source.txt")
        (self.source / "new test.txt").write_text("test added")
        result = self.finish()
        self.assertTrue(result["ok"])
        self.assertEqual(result["changedFiles"], ["new test.txt", "source.txt"])
        self.assertEqual(result["untrackedFiles"], ["new test.txt"])
        self.assertNotIn(skill.TASK_FILE, result["diffStat"])
        self.assertFalse((self.source / skill.TASK_FILE).exists())

    def test_committed_changes_are_compared_against_original_base(self):
        self.prepare()
        (self.source / "source.txt").write_text("fixed\n")
        skill.git(self.source, "add", "source.txt")
        skill.git(self.source, "commit", "-m", "Fix parser")
        result = self.finish()
        self.assertEqual(result["changedFiles"], ["source.txt"])

    def test_staged_or_previously_committed_task_blocks_finish(self):
        self.prepare()
        skill.git(self.source, "add", skill.TASK_FILE)
        with self.assertRaisesRegex(skill.WorkflowError, "staged or committed"):
            self.finish()
        skill.git(self.source, "commit", "-m", "Accidentally add task")
        saved = (self.source / skill.TASK_FILE).read_bytes()
        skill.git(self.source, "rm", skill.TASK_FILE)
        skill.git(self.source, "commit", "-m", "Remove task")
        (self.source / skill.TASK_FILE).write_bytes(saved)
        with self.assertRaisesRegex(skill.WorkflowError, "staged or committed"):
            self.finish()

    def test_changed_branch_or_task_symlink_blocks_finish(self):
        self.prepare()
        skill.git(self.source, "switch", "-c", "other")
        with self.assertRaisesRegex(skill.WorkflowError, "branch differs"):
            self.finish()
        task = self.source / skill.TASK_FILE
        target = self.root / "task.json"
        task.rename(target)
        task.symlink_to(target)
        with self.assertRaises(skill.WorkflowError):
            self.finish()
        self.assertTrue(target.exists())

    def test_check_side_effect_cannot_change_task_or_stage_it(self):
        self.prepare()
        with self.assertRaisesRegex(skill.WorkflowError, "staged or committed"):
            self.finish(["git add .issue-finder-task.json"])
        self.assertTrue((self.source / skill.TASK_FILE).exists())

    def test_no_changes_and_missing_executable_do_not_claim_completion(self):
        self.prepare()
        result = self.finish([check_command("assert open('source.txt').read() == 'original\\n'")])
        self.assertEqual(result["status"], "no_changes")
        result = self.finish([str(self.root / "missing-check")])
        self.assertEqual(result["checks"][0]["exitCode"], 127)
        self.assertFalse(result["ok"])
        self.assertTrue((self.source / skill.TASK_FILE).exists())

    def test_check_timeout_and_output_are_bounded(self):
        self.prepare()
        command = check_command("import time; print('x' * 20000, flush=True); time.sleep(10)")
        result = self.finish([command], timeout=1)
        self.assertEqual(result["checks"][0]["exitCode"], 124)
        self.assertTrue(result["checks"][0]["timedOut"])
        self.assertLessEqual(len(result["checks"][0]["outputTail"]), 6000)
        self.assertTrue((self.source / skill.TASK_FILE).exists())

    def test_commands_are_not_shell_expanded_and_unusual_filenames_survive(self):
        self.prepare()
        name = " space\nname.txt"
        (self.source / name).write_text("fixed")
        check = check_command("import sys; assert sys.argv[1:] == ['$(touch INJECTED)', ';']")
        result = self.finish([check + " '$(touch INJECTED)' ';'"])
        self.assertTrue(result["ok"])
        self.assertEqual(result["changedFiles"], [name])
        self.assertFalse((self.source / "INJECTED").exists())

    def test_staged_changes_are_reported_even_when_worktree_reverses_them(self):
        self.prepare()
        (self.source / "source.txt").write_text("staged change\n")
        skill.git(self.source, "add", "source.txt")
        (self.source / "source.txt").write_text("original\n")
        result = self.finish([check_command("assert open('source.txt').read() == 'original\\n'")])
        self.assertEqual(result["changedFiles"], ["source.txt"])
        self.assertEqual(result["stagedFiles"], ["source.txt"])

    def test_copied_skill_runs_full_cli_flow_without_source_checkout_or_rust(self):
        installed = self.root / "installed-skill"
        shutil.copytree(SCRIPT.parents[1], installed)
        executable = installed / "scripts/issue_finder.py"
        commands = self.root / "bin"
        commands.mkdir()
        fake_gh = commands / "gh"
        responses = {"repos/example/project": repository(),
                     "search/issues": {"items": [issue()], "total_count": 1},
                     "repos/example/project/issues/1": issue(),
                     "repos/example/project/issues/1/comments": [[]],
                     "repos/example/project/issues/1/timeline": [[]]}
        fake_gh.write_text(f"#!{sys.executable}\nimport json, sys\n"
                           f"responses = json.loads({json.dumps(responses)!r})\n"
                           "endpoint = next(arg for arg in sys.argv if arg in responses)\n"
                           "print(json.dumps(responses[endpoint]))\n")
        fake_gh.chmod(0o755)

        def invoke(*args):
            result = subprocess.run([sys.executable, str(executable), *args, "--json"],
                                    cwd=self.source, text=True, capture_output=True,
                                    env={**os.environ, "PATH": str(commands) + os.pathsep + os.environ["PATH"]})
            payload = json.loads(result.stdout)
            self.assertEqual(result.returncode == 0, payload["ok"])
            self.assertEqual(result.stderr, "")
            return payload

        candidates = invoke("scout", "--limit", "2", "--scan-limit", "5")
        self.assertEqual(candidates["candidates"][0]["number"], 1)
        prepared = invoke("prepare", "--issue", "example/project#1", "--workspace-root", str(self.contributions))
        self.assertTrue(prepared["ok"])
        args = ("finish", "--workspace", str(self.source), "--check",
                check_command("assert open('source.txt').read() == 'fixed\\n'"))
        self.assertEqual(invoke(*args)["status"], "validation_failed")
        (self.source / "source.txt").write_text("fixed\n")
        self.assertEqual(invoke(*args)["status"], "completed")
        self.assertFalse((self.source / skill.TASK_FILE).exists())


if __name__ == "__main__":
    unittest.main()
