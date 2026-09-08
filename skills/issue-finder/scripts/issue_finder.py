#!/usr/bin/env python3
"""Small, standalone GitHub issue workflow. Python standard library only."""

import argparse
import datetime as dt
import json
import os
from pathlib import Path
import re
import shlex
import signal
import subprocess
import sys
import tempfile
import uuid

TASK_FILE = ".issue-finder-task.json"
REPO_PATTERN = r"[A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9_.-]+"
MAINTAINERS = {"OWNER", "MEMBER", "COLLABORATOR"}


class WorkflowError(Exception):
    pass


def run(argv, cwd=None, timeout=120, check=True):
    try:
        result = subprocess.run(
            argv, cwd=cwd, text=True, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, timeout=timeout,
            env={**os.environ, "GIT_TERMINAL_PROMPT": "0", "GH_PROMPT_DISABLED": "1"},
        )
    except FileNotFoundError as exc:
        raise WorkflowError(f"Required executable is unavailable: {argv[0]}") from exc
    except subprocess.TimeoutExpired as exc:
        raise WorkflowError(f"{argv[0]} timed out after {timeout}s; no success recorded") from exc
    if check and result.returncode:
        raise WorkflowError(f"{argv[0]} failed: {result.stderr.strip()[-2000:]}")
    return result


def git(workspace, *args, check=True):
    output = run(["git", "-C", str(workspace), *args], check=check).stdout
    return output if "-z" in args else output.strip()


def repo_name(value):
    if not re.fullmatch(REPO_PATTERN, value) or value.split("/")[1] in {".", ".."}:
        raise WorkflowError("Expected a github.com repository in owner/repo form")
    return value


def remote_repo(url):
    match = re.fullmatch(
        r"(?:https://github\.com/|git@github\.com:|ssh://git@github\.com/)(.+?)(?:\.git)?/?",
        url,
    )
    if not match:
        return None
    try:
        return repo_name(match[1])
    except WorkflowError:
        return None


def git_root(path):
    result = run(["git", "-C", str(path), "rev-parse", "--show-toplevel"], check=False)
    return Path(result.stdout.strip()).resolve() if result.returncode == 0 else None


def repository_remotes(workspace):
    if not workspace:
        return []
    return [remote_repo(git(workspace, "config", "--get", f"remote.{name}.url", check=False))
            for name in ("upstream", "origin")]


def resolve_repo(value):
    if value:
        return repo_name(value)
    for name in repository_remotes(git_root(Path.cwd())):
        if name:
            return name
    raise WorkflowError("No GitHub repository inferred; provide --repo owner/repo")


def parse_issue(value):
    match = re.fullmatch(rf"({REPO_PATTERN})#([1-9][0-9]*)", value)
    if not match:
        match = re.fullmatch(rf"https://github\.com/({REPO_PATTERN})/issues/([1-9][0-9]*)/?", value)
    if not match:
        raise WorkflowError("Expected owner/repo#123 or a github.com issue URL")
    return repo_name(match[1]), int(match[2])


def api(endpoint, fields=None, paginate=False):
    argv = ["gh", "api", "--hostname", "github.com", "--method", "GET",
            "-H", "Accept: application/vnd.github+json",
            "-H", "X-GitHub-Api-Version: 2022-11-28", endpoint]
    for key, value in (fields or {}).items():
        argv.extend(["-f", f"{key}={value}"])
    if paginate:
        argv.extend(["--paginate", "--slurp"])
    try:
        data = json.loads(run(argv).stdout)
        return [item for page in data for item in page] if paginate else data
    except (ValueError, TypeError) as exc:
        raise WorkflowError("GitHub returned an invalid JSON response") from exc


def issue_evidence(repo, number):
    endpoint = f"repos/{repo}/issues/{number}"
    issue = api(endpoint)
    comments = api(endpoint + "/comments", {"per_page": 100}, paginate=True)
    timeline = api(endpoint + "/timeline", {"per_page": 100}, paginate=True)
    return issue, comments, timeline


def age_days(timestamp):
    try:
        then = dt.datetime.fromisoformat(timestamp.replace("Z", "+00:00"))
        return max(0, (dt.datetime.now(dt.timezone.utc) - then).days)
    except (ValueError, TypeError, AttributeError):
        return None


def assess(issue, repository, comments, timeline, query=""):
    """Only cheap observable signals; Codex owns semantic selection and review."""
    labels = {label["name"].lower() for label in issue.get("labels", [])}
    body = (issue.get("body") or "").strip()
    rejected = []
    if issue.get("state") != "open" or "pull_request" in issue:
        rejected.append("not an open issue")
    if repository.get("archived") or repository.get("disabled"):
        rejected.append("repository is archived or disabled")
    if issue.get("locked"):
        rejected.append("issue is locked")
    if issue.get("assignees") or issue.get("assignee"):
        rejected.append("issue already has an assignee")
    if labels & {"discussion", "question", "invalid", "duplicate", "wontfix", "wont-fix", "in progress", "in-progress"}:
        rejected.append("discussion, non-actionable, or already in-progress label")
    if any(re.search(r"(?:^|[^a-z])blocked(?:$|[^a-z])", label) for label in labels):
        rejected.append("issue has an explicit blocked label")
    if len(body) < 40:
        rejected.append("insufficient issue description")
    if issue.get("title", "").strip().lower() == "dependency dashboard":
        rejected.append("dependency dashboard is an automation index, not an implementation task")
    for event in timeline:
        source = (event.get("source") or {}).get("issue") or {}
        if source.get("pull_request") and source.get("state") == "open":
            rejected.append("open pull request references this issue")
            break
    for comment in comments:
        age = age_days(comment.get("updated_at"))
        if age is not None and age <= 30 and re.search(
            r"\b(?:i(?:'m| am) (?:currently )?working on|i(?:'ll| will) (?:take|work on)|working on (?:this|it)|i have (?:opened|submitted) (?:a |the )?pr)\b",
            comment.get("body") or "", re.I,
        ):
            rejected.append("recent comment claims active work")
            break
    score, reasons, risks = 30, [], []
    for label, weight in (("good first issue", 15), ("help wanted", 12), ("bug", 8)):
        if label in labels:
            score += weight
            reasons.append(f"{label} label")
    if re.search(r"reproduc|steps to|expected|actual|acceptance|```", body, re.I):
        score += 10
        reasons.append("description contains reproduction or expected-behavior markers")
    else:
        risks.append("reproduction and acceptance criteria need semantic review")
    for timestamp, label in ((issue.get("updated_at"), "issue updated recently"),
                             (repository.get("pushed_at"), "repository pushed recently")):
        age = age_days(timestamp)
        if age is not None and age <= 90:
            score += 5
            reasons.append(label)
        else:
            risks.append("activity is stale or unknown: " + label)
    if any(c.get("author_association") in MAINTAINERS for c in comments):
        score += 5
        reasons.append("maintainer participated in discussion")
    if len(comments) > 20:
        score -= 5
        risks.append("long discussion requires review")
    if any("tracking" in label for label in labels):
        risks.append("tracking issue may span multiple tasks; identify a concrete actionable change")
    if query and any(word.lower() in (issue.get("title", "") + " " + body).lower()
                     for word in query.split() if ":" not in word):
        score += 5
        reasons.append("matches requested search terms")
    risks.append("code scope, validation, and unlinked competition require Codex review")
    return {"score": min(100, max(0, score)), "reasons": reasons, "risks": risks,
            "rejectedReasons": rejected}


def comment_summary(comment, excerpt=False):
    body = comment.get("body") or ""
    return {"author": (comment.get("user") or {}).get("login"),
            "association": comment.get("author_association"),
            "body": body[:800] if excerpt else body,
            "url": comment.get("html_url"), "updatedAt": comment.get("updated_at")}


def scout(args):
    repo = resolve_repo(args.repo)
    repository = api(f"repos/{repo}")
    if repository.get("archived") or repository.get("disabled"):
        return {"ok": False, "status": "no_candidates", "repo": repo,
                "candidates": [], "warnings": ["Repository is archived or disabled"]}
    response = api("search/issues", {"q": f"repo:{repo} is:issue is:open {args.query}",
                                     "sort": "updated", "order": "desc", "per_page": args.scan_limit})
    candidates, rejected, warnings = [], [], []
    if response.get("incomplete_results"):
        warnings.append("GitHub search returned incomplete results")
    for item in response.get("items", []):
        # Search qualifiers must not let a query broaden the requested repository.
        if item.get("repository_url", "").lower() != f"https://api.github.com/repos/{repo}".lower():
            continue
        number = item["number"]
        initial = assess(item, repository, [], [], args.query)
        if initial["rejectedReasons"]:
            rejected.append({"number": number, "reasons": initial["rejectedReasons"]})
            continue
        try:
            issue, comments, timeline = issue_evidence(repo, number)
        except WorkflowError as exc:
            warnings.append(f"#{number} excluded: could not verify discussion/competition: {exc}")
            continue
        selection = assess(issue, repository, comments, timeline, args.query)
        if selection["rejectedReasons"]:
            rejected.append({"number": number, "reasons": selection["rejectedReasons"]})
            continue
        candidates.append({"repo": repo, "number": number, "title": issue["title"],
                           "url": issue["html_url"], "labels": [x["name"] for x in issue.get("labels", [])],
                           **selection, "bodyExcerpt": (issue.get("body") or "")[:2000],
                           "commentCount": len(comments), "updatedAt": issue.get("updated_at"),
                           "recentComments": [comment_summary(c, excerpt=True) for c in comments[-3:]]})
    candidates.sort(key=lambda c: (-c["score"], c["number"]))
    return {"ok": bool(candidates), "status": "candidates_found" if candidates else "no_candidates",
            "repo": repo, "candidates": candidates[:args.limit], "rejected": rejected,
            "warnings": warnings, "scanned": len(response.get("items", [])),
            "searchTotal": response.get("total_count"), "scanLimit": args.scan_limit}


def ensure_no_task(workspace):
    path = workspace / TASK_FILE
    if path.exists() or path.is_symlink() or git(workspace, "ls-files", "--", TASK_FILE):
        raise WorkflowError(f"Task file already exists or is tracked in {workspace}; resume it first")


def contribution_root(value):
    root = Path(value).expanduser().resolve()
    ancestor = root
    while not ancestor.exists():
        ancestor = ancestor.parent
    if git_root(ancestor):
        raise WorkflowError("Contribution root must be outside existing Git repositories")
    root.mkdir(parents=True, exist_ok=True)
    return root


def fetch_base(workspace, repo, default_branch):
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9/_.-]*", default_branch):
        raise WorkflowError("Unsupported default branch name")
    git(workspace, "fetch", "--no-tags", f"https://github.com/{repo}.git",
        f"refs/heads/{default_branch}")
    return git(workspace, "rev-parse", "FETCH_HEAD^{commit}")


def prepare_workspace(repo, number, repository, root_value):
    current = git_root(Path.cwd())
    matches = any(name and name.lower() == repo.lower() for name in repository_remotes(current))
    suffix = uuid.uuid4().hex[:8]
    branch = f"fuyue/issue-{number}-{suffix}"
    if matches:
        ensure_no_task(current)
        if not git(current, "status", "--porcelain", "--untracked-files=all"):
            base = git(current, "rev-parse", "HEAD")
            git(current, "switch", "-c", branch)
            return current, branch, base
        source = current
    else:
        root = contribution_root(root_value)
        owner, name = repo.split("/")
        source = root / owner / name
        if source.is_symlink() or source.resolve() != source:
            raise WorkflowError("Refusing a symlinked repository cache")
        if not source.exists():
            source.parent.mkdir(parents=True, exist_ok=True)
            run(["git", "clone", "--", f"https://github.com/{repo}.git", str(source)], timeout=600)
            ensure_no_task(source)
            base = git(source, "rev-parse", "HEAD")
            git(source, "switch", "-c", branch)
            return source, branch, base
        if git_root(source) != source or not any(
            name and name.lower() == repo.lower() for name in repository_remotes(source)
        ):
            raise WorkflowError("Existing cache path is not a checkout of the requested repository")
    root = contribution_root(root_value)
    base = fetch_base(source, repo, repository["default_branch"])
    workspace = root / "workspaces" / f"{repo.replace('/', '-')}-{number}-{suffix}"
    if workspace.resolve() != workspace:
        raise WorkflowError("Refusing a symlinked worktree destination")
    workspace.parent.mkdir(parents=True, exist_ok=True)
    git(source, "worktree", "add", "-b", branch, str(workspace), base)
    ensure_no_task(workspace)
    return workspace, branch, base


def repository_hints(workspace):
    paths = git(workspace, "ls-files", "-z").split("\0")
    instructions = [p for p in paths if Path(p).name.lower() in {"agents.md", "contributing.md", "contributing"}]
    manifests = [p for p in paths if Path(p).name in {
        "Cargo.toml", "package.json", "pyproject.toml", "setup.cfg", "go.mod", "Makefile", "justfile"}]
    checks = []
    if "Cargo.toml" in manifests:
        checks.extend(["cargo test", "cargo clippy --all-targets -- -D warnings"])
    if "go.mod" in manifests:
        checks.append("go test ./...")
    return {"instructionFiles": instructions, "manifests": manifests, "suggestedChecks": checks}


def prepare(args):
    repo, number = parse_issue(args.issue)
    repository = api(f"repos/{repo}")
    issue, comments, timeline = issue_evidence(repo, number)
    selection = assess(issue, repository, comments, timeline)
    if selection["rejectedReasons"]:
        return {"ok": False, "status": "candidate_unavailable", "issue": args.issue, **selection}
    workspace, branch, base = prepare_workspace(repo, number, repository, args.workspace_root)
    task = {"version": 1,
            "issue": {"repo": repo, "number": number, "title": issue["title"], "url": issue["html_url"],
                      "body": issue.get("body") or "", "labels": [x["name"] for x in issue.get("labels", [])],
                      "relevantComments": [comment_summary(c) for c in comments[-20:]],
                      "commentCount": len(comments), "commentsTruncated": len(comments) > 20},
            "selection": selection,
            "workspace": {"path": str(workspace), "branch": branch, "baseCommit": base},
            "repo": repository_hints(workspace)}
    # Exclusive creation prevents replacing user files or following symlinks.
    with (workspace / TASK_FILE).open("x", encoding="utf-8") as stream:
        json.dump(task, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    return {"ok": True, "status": "prepared", "workspace": task["workspace"],
            "taskFile": str(workspace / TASK_FILE), "repo": task["repo"], "selection": selection}


def load_task(workspace):
    path = workspace / TASK_FILE
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 8_000_000:
        raise WorkflowError("Expected a regular, bounded .issue-finder-task.json in the workspace")
    try:
        task = json.loads(path.read_text(encoding="utf-8"))
        metadata = task["workspace"]
        repo_name(task["issue"]["repo"])
        valid = (task["version"] == 1 and Path(metadata["path"]).resolve() == workspace
                 and isinstance(metadata["branch"], str)
                 and re.fullmatch(r"[0-9a-f]{40,64}", metadata["baseCommit"])
                 and isinstance(task["issue"]["number"], int) and task["issue"]["number"] > 0)
    except (KeyError, ValueError, TypeError) as exc:
        raise WorkflowError("Invalid task JSON; do not reconstruct or edit it to bypass validation") from exc
    if not valid or git_root(workspace) != workspace:
        raise WorkflowError("Task does not match this Git workspace")
    return task


def validate_task(workspace, task):
    metadata = task["workspace"]
    if git(workspace, "branch", "--show-current") != metadata["branch"]:
        raise WorkflowError("Current branch differs from the prepared task branch")
    git(workspace, "merge-base", "--is-ancestor", metadata["baseCommit"], "HEAD")
    if (git(workspace, "ls-files", "--", TASK_FILE)
            or git(workspace, "log", "--format=%H", f"{metadata['baseCommit']}..HEAD", "--", TASK_FILE)):
        raise WorkflowError("Task JSON is staged or committed; remove it from the contribution before finishing")


def run_check(command, workspace, timeout):
    try:
        argv = shlex.split(command)
    except ValueError as exc:
        raise WorkflowError(f"Invalid --check quoting: {exc}") from exc
    if not argv:
        raise WorkflowError("Validation command must not be empty")
    # No shell expansion. Log to a temporary file so verbose checks cannot fill memory.
    with tempfile.TemporaryFile() as output:
        try:
            process = subprocess.Popen(argv, cwd=workspace, stdout=output, stderr=subprocess.STDOUT,
                                       start_new_session=(os.name == "posix"))
            try:
                code = process.wait(timeout=timeout)
                timed_out = False
            except subprocess.TimeoutExpired:
                if os.name == "posix":
                    os.killpg(process.pid, signal.SIGKILL)
                else:
                    process.kill()
                process.wait()
                code, timed_out = 124, True
        except OSError as exc:
            return {"command": command, "exitCode": 127, "outputTail": str(exc), "timedOut": False}
        length = output.tell()
        output.seek(max(0, length - 6000))
        return {"command": command, "exitCode": code, "timedOut": timed_out,
                "outputTail": output.read().decode("utf-8", errors="replace")}


def finish(args):
    workspace = Path(args.workspace).expanduser().resolve()
    task = load_task(workspace)
    validate_task(workspace, task)
    checks = [run_check(command, workspace, args.check_timeout) for command in args.check]
    if load_task(workspace) != task:
        raise WorkflowError("Task JSON changed during validation; no success recorded")
    validate_task(workspace, task)
    base = task["workspace"]["baseCommit"]
    tracked = git(workspace, "diff", "--no-ext-diff", "--no-textconv", "--name-only", "-z", base, "--").split("\0")
    staged = git(workspace, "diff", "--cached", "--no-ext-diff", "--no-textconv", "--name-only", "-z", "--").split("\0")
    untracked = git(workspace, "ls-files", "--others", "--exclude-standard", "-z").split("\0")
    changed = sorted(set(tracked + staged + untracked) - {"", TASK_FILE})
    failed = any(check["exitCode"] != 0 for check in checks)
    status = "validation_failed" if failed else "completed" if changed else "no_changes"
    result = {"ok": status == "completed", "status": status,
              "issue": f"{task['issue']['repo']}#{task['issue']['number']}",
              "workspace": str(workspace), "branch": task["workspace"]["branch"], "baseCommit": base,
              "changedFiles": changed, "stagedFiles": sorted(set(staged) - {"", TASK_FILE}),
              "untrackedFiles": sorted(set(untracked) - {"", TASK_FILE}),
              "diffStat": git(workspace, "diff", "--no-ext-diff", "--no-textconv", "--stat", base,
                              "--", ".", f":(exclude){TASK_FILE}"),
              "checks": checks}
    if result["ok"]:
        (workspace / TASK_FILE).unlink()
    return result


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise WorkflowError(message)


def positive_int(value):
    try:
        number = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("expected a positive integer") from exc
    if number <= 0:
        raise argparse.ArgumentTypeError("expected a positive integer")
    return number


def main(argv=None):
    parser = Parser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    discover = commands.add_parser("scout", help="Find a small set of unclaimed GitHub issues")
    discover.add_argument("--repo", help="owner/repo; inferred from current checkout if omitted")
    discover.add_argument("--query", default="", help="Additional GitHub search terms")
    discover.add_argument("--limit", type=positive_int, default=8)
    discover.add_argument("--scan-limit", type=positive_int, default=30)
    discover.set_defaults(handler=scout)
    prep = commands.add_parser("prepare", help="Prepare an issue branch/worktree and one task JSON")
    prep.add_argument("--issue", required=True)
    prep.add_argument("--workspace-root", default=os.environ.get("ISSUE_FINDER_WORKSPACE_ROOT", "~/Code/contributions"))
    prep.set_defaults(handler=prepare)
    complete = commands.add_parser("finish", help="Run checks and report changes; never commit or publish")
    complete.add_argument("--workspace", required=True)
    complete.add_argument("--check", action="append", required=True, help="Executable and arguments, without a shell")
    complete.add_argument("--check-timeout", type=positive_int, default=600)
    complete.set_defaults(handler=finish)
    for command in (discover, prep, complete):
        command.add_argument("--json", action="store_true", help="Emit JSON (also the default)")
    try:
        args = parser.parse_args(argv)
        if args.command == "scout" and not 1 <= args.limit <= args.scan_limit <= 100:
            raise WorkflowError("Require 1 <= --limit <= --scan-limit <= 100")
        result = args.handler(args)
    except (WorkflowError, OSError) as exc:
        result = {"ok": False, "status": "blocked", "error": str(exc)}
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
