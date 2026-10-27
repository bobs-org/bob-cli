#!/usr/bin/env python3
"""Offline behavior tests for macos-ci-check; never invoke the live gate."""

from __future__ import annotations

import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest


HERE = Path(__file__).resolve().parent
SCRIPT_SRC = HERE / "macos-ci-check"
LOADER = importlib.machinery.SourceFileLoader("macos_ci_check", str(SCRIPT_SRC))
SPEC = importlib.util.spec_from_loader("macos_ci_check", LOADER)
GATE = importlib.util.module_from_spec(SPEC)
LOADER.exec_module(GATE)

if HERE.name == "Scripts":
    REPO = "bobs-org/bob-mac-capture"
    REL_SCRIPT = Path("Scripts/macos-ci-check")
else:
    REPO = "bobs-org/bob-cli"
    REL_SCRIPT = Path("scripts/dev/macos-ci-check")

PROFILE = GATE.PROFILES[REPO]
ORIGIN = f"https://github.com/{REPO}.git"
HEAD = "a" * 40
SNAP = "b" * 40
TREE = "c" * 40
RUN_ID = "40485333363"

GIT_STUB = r'''
import json, os, pathlib, sys, time
root = pathlib.Path(os.environ["TEST_ROOT"])
args = sys.argv[1:]
cwd = None
if args[:1] == ["-C"]:
    cwd = args[1]
    args = args[2:]
with (root / "git.log").open("a") as log:
    log.write(json.dumps({"cwd": cwd, "args": args, "index": os.environ.get("GIT_INDEX_FILE")}) + "\n")
mode = os.environ.get("GIT_MODE", "")
state_path = root / "git-state.json"
state = json.loads(state_path.read_text()) if state_path.exists() else {}
forbidden = {"checkout", "switch", "stash", "reset", "update-ref", "branch", "commit"}
if args and args[0] in forbidden:
    sys.stderr.write("forbidden git command " + args[0])
    sys.exit(99)
if mode == "stall" or (mode == "stall-push" and args[:1] == ["push"] and not any(a.startswith(":") for a in args)):
    time.sleep(float(os.environ.get("STALL_SECONDS", "8")))
if args[:2] == ["rev-parse", "--show-toplevel"]:
    print(os.environ["GATE_ROOT"])
elif args[:1] == ["rev-parse"] and "HEAD" in args and "MERGE_HEAD" not in args:
    print(os.environ.get("HEAD_SHA", "a" * 40))
elif "MERGE_HEAD" in args:
    sys.exit(0 if mode == "merge" else 1)
elif args[:1] == ["config"] and "--get-all" in args:
    key = args[-1]
    if key == "remote.origin.pushurl":
        sys.exit(1)
    if key == "remote.origin.url":
        print(os.environ.get("ORIGIN_URL", ""), end="" if str(os.environ.get("ORIGIN_URL", "")).endswith("\n") else "\n")
        sys.exit(0)
    sys.exit(1)
elif args[:1] == ["status"]:
    print(os.environ.get("GIT_STATUS", ""), end="" if os.environ.get("GIT_STATUS", "").endswith("\n") or not os.environ.get("GIT_STATUS") else "\n")
elif args[:1] == ["read-tree"]:
    pathlib.Path(os.environ["GIT_INDEX_FILE"]).write_text("index")
elif args[:1] == ["add"]:
    pass
elif args[:1] == ["write-tree"]:
    print(os.environ.get("TREE_SHA", "c" * 40))
elif args[:1] == ["commit-tree"]:
    print(os.environ.get("SNAP_SHA", "b" * 40))
elif args[:1] == ["push"]:
    dests = [a for a in args if ":" in a and not a.startswith("--")]
    if any(d.endswith("master") or d.endswith("refs/heads/master") for d in dests if not d.startswith(":")):
        sys.stderr.write("refusing master push")
        sys.exit(99)
    deleting = any(a.startswith(":") or a.startswith("refs/heads/") and False for a in args)
    deleting = any(a.startswith(":") for a in args)
    if deleting:
        state["delete_attempts"] = state.get("delete_attempts", 0) + 1
        state_path.write_text(json.dumps(state))
        if mode == "delete-fail":
            sys.stderr.write("failed to delete ref")
            sys.exit(1)
        if mode == "delete-retry" and state["delete_attempts"] == 1:
            sys.stderr.write("HTTP 502 temporary")
            sys.exit(1)
        state["deleted"] = True
        state_path.write_text(json.dumps(state))
        sys.exit(0)
    if mode == "push-fail":
        sys.stderr.write("unable to push")
        sys.exit(1)
    (root / "pushed.json").write_text(json.dumps(args))
    sys.exit(0)
elif args[:1] == ["ls-remote"]:
    if mode == "ls-remote-fail":
        sys.stderr.write("could not read remote")
        sys.exit(128)
    if mode == "delete-absent" or state.get("deleted"):
        sys.exit(0)
    sha = "d" * 40 if mode == "delete-other-sha" else os.environ.get("SNAP_SHA", "b" * 40)
    print(sha + "\t" + args[-1])
else:
    sys.stderr.write("unhandled git " + " ".join(args))
    sys.exit(99)
'''

GH_STUB = r'''
import json, os, pathlib, sys, time
root = pathlib.Path(os.environ["TEST_ROOT"])
args = sys.argv[1:]
with (root / "gh.log").open("a") as log:
    log.write(json.dumps(args) + "\n")
mode = os.environ.get("GH_MODE", "success")
state_path = root / "gh-state.json"
state = json.loads(state_path.read_text()) if state_path.exists() else {}
if mode == "stall" and args[:2] == ["run", "list"]:
    time.sleep(float(os.environ.get("STALL_SECONDS", "8")))
if args[:2] == ["auth", "status"] or args[:1] == ["auth"]:
    if mode == "auth":
        sys.stderr.write("authentication required\n")
        sys.exit(4)
    sys.exit(0)

repo = os.environ["GATE_REPO"]
profile_job = os.environ["GATE_JOB"]
required = json.loads(os.environ["GATE_REQUIRED"])
sha = os.environ.get("SNAP_SHA", "b" * 40)
branch_prefix = "ci/precommit/"

def emit_json(value):
    text = value if isinstance(value, str) else json.dumps(value)
    if os.environ.get("GH_COLOR_JSON") == "1":
        text = "".join(
            ("\x1b[1;37m" + ch + "\x1b[m") if ch in "[]{}" else ch for ch in text
        )
    print(text)

def arg_value(flag, default=""):
    if flag in args:
        return args[args.index(flag) + 1]
    return default

if "--commit" in args:
    sha = arg_value("--commit", sha)
    state["sha"] = sha
if "--branch" in args:
    state["branch"] = arg_value("--branch")
if state.get("sha"):
    sha = state["sha"]
state_path.write_text(json.dumps(state))

def step(name, conclusion="success", status="completed"):
    return {"name": name, "conclusion": conclusion, "status": status, "number": 1}

def required_steps(conclusion_map=None, extra=None):
    conclusion_map = conclusion_map or {}
    steps = [step("Set up job")]
    for name in required:
        steps.append(step(name, conclusion_map.get(name, "success")))
    steps.extend(extra or [])
    steps.append(step("Complete job"))
    return steps

def macos_job(conclusion="success", steps=None, status="completed"):
    return {
        "name": profile_job,
        "conclusion": conclusion,
        "status": status,
        "databaseId": 11,
        "steps": steps if steps is not None else required_steps(),
    }

def run_payload(head_sha=None, head_branch=None, event="push", status="completed",
                conclusion="success", jobs=None):
    return {
        "databaseId": int(os.environ.get("RUN_ID", "40485333363")),
        "headSha": head_sha or sha,
        "headBranch": head_branch or state.get("branch") or os.environ.get("RUN_BRANCH", ""),
        "event": event,
        "status": status,
        "conclusion": conclusion,
        "url": "https://github.com/" + repo + "/actions/runs/" + os.environ.get("RUN_ID", "40485333363"),
        "name": "CI",
        "workflowName": "CI",
        "jobs": jobs if jobs is not None else [
            {"name": "lint", "conclusion": "success", "status": "completed", "steps": []},
            macos_job(),
        ],
    }

def matching_branch(args):
    if "--branch" in args:
        return args[args.index("--branch") + 1]
    return state.get("branch") or os.environ.get("RUN_BRANCH", "ci/precommit/test")

if args[:2] == ["run", "list"]:
    state["list_count"] = state.get("list_count", 0) + 1
    branch = matching_branch(args)
    state["branch"] = branch
    state_path.write_text(json.dumps(state))
    os.environ["RUN_BRANCH"] = branch
    if mode == "malformed":
        print("{not json")
        sys.exit(0)
    if mode == "api-error":
        sys.stderr.write("HTTP 502 Bad Gateway")
        sys.exit(1)
    if mode == "empty":
        emit_json("[]")
        sys.exit(0)
    if mode == "delayed" and state["list_count"] < int(os.environ.get("DELAY_COUNT", "3")):
        emit_json("[]")
        sys.exit(0)
    wrong = run_payload(head_sha="f" * 40, head_branch="master", event="pull_request")
    right = run_payload(head_branch=branch)
    if mode == "wrong-identity":
        emit_json([wrong])
    elif mode == "mixed-identity":
        emit_json([wrong, right])
    else:
        emit_json([right])
    sys.exit(0)

if args[:2] == ["run", "view"]:
    branch = state.get("branch") or os.environ.get("RUN_BRANCH") or matching_branch(args)
    if "--log-failed" in args:
        if mode == "log-unavailable":
            sys.stderr.write("HTTP 500")
            sys.exit(1)
        if mode == "fail-rust":
            print("test native::foo::bar ... FAILED\n\nfailures:\n    native::foo::bar\n")
        elif mode == "fail-swift":
            print("Test Case '-[BobMacCaptureTests.FooTests testBar]' failed (0.012 seconds).\n")
            print('✘ Test "launch smoke" failed after 1 retry\n')
        elif mode == "fail-lint-skip":
            print("error: cannot compile sample.swift\n")
        else:
            print("test example ... FAILED\n")
        sys.exit(0)
    if mode == "queued":
        payload = run_payload(head_branch=branch, status="in_progress", conclusion="", jobs=[
            macos_job(conclusion="", status="in_progress", steps=[step("Set up job", "", "in_progress")]),
        ])
    elif mode == "cancelled":
        payload = run_payload(head_branch=branch, conclusion="cancelled", jobs=[
            macos_job("cancelled", required_steps({name: "cancelled" for name in required})),
        ])
    elif mode == "missing-job":
        payload = run_payload(head_branch=branch, jobs=[
            {"name": "lint", "conclusion": "success", "status": "completed", "steps": []},
        ])
    elif mode == "missing-step":
        steps = [step("Set up job")] + [step(name) for name in required[:-1]]
        payload = run_payload(head_branch=branch, jobs=[macos_job(steps=steps)])
    elif mode == "skip-optional":
        extra = [step("Upload render fixtures", "skipped"), step("Install full-test tools", "skipped")]
        payload = run_payload(head_branch=branch, jobs=[macos_job(steps=required_steps(extra=extra))])
    elif mode == "fail-rust" or mode == "fail-swift":
        failed = "Run tests" if "Run tests" in required else "Test"
        cmap = {failed: "failure"}
        later = False
        steps = [step("Set up job")]
        for name in required:
            if name == failed:
                steps.append(step(name, "failure"))
                later = True
            elif later:
                steps.append(step(name, "skipped"))
            else:
                steps.append(step(name))
        payload = run_payload(head_branch=branch, conclusion="failure", jobs=[macos_job("failure", steps)])
    elif mode == "fail-lint-skip":
        first = required[0]
        cmap = {first: "failure"}
        steps = []
        seen = False
        for name in ["Set up job", *required]:
            if name == first:
                steps.append(step(name, "failure"))
                seen = True
            elif seen and name in required:
                steps.append(step(name, "skipped"))
            else:
                steps.append(step(name))
        payload = run_payload(head_branch=branch, conclusion="failure", jobs=[macos_job("failure", steps)])
    elif mode == "fail-other-job":
        payload = run_payload(head_branch=branch, conclusion="failure", jobs=[
            {"name": "lint", "conclusion": "failure", "status": "completed", "steps": [step("Format", "failure")]},
            macos_job(),
        ])
    else:
        payload = run_payload(head_branch=branch)
    emit_json(payload)
    sys.exit(0)

if args[:2] == ["run", "cancel"]:
    (root / "cancelled.json").write_text(json.dumps(args))
    sys.exit(0)

sys.stderr.write("unhandled gh " + " ".join(args))
sys.exit(99)
'''


def json_lines(path: Path) -> list:
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


class ParserTests(unittest.TestCase):
    def test_github_urls_and_credential_stripping(self) -> None:
        urls = {
            "https://github.com/bobs-org/bob-cli.git": "bobs-org/bob-cli",
            "git@github.com:bobs-org/bob-mac-capture.git": "bobs-org/bob-mac-capture",
            "ssh://git@github.com/bobs-org/bob-cli": "bobs-org/bob-cli",
            "https://user:token@github.com/bobs-org/bob-cli.git": "bobs-org/bob-cli",
        }
        for url, repo in urls.items():
            self.assertEqual(GATE.github_repo_from_url(url), repo)
            self.assertNotIn("token", GATE.strip_credentials(url))
        self.assertIsNone(GATE.github_repo_from_url("https://github.com/other/repo.git"))
        self.assertIsNone(GATE.github_repo_from_url("https://gitlab.com/bobs-org/bob-cli.git"))

    def test_parse_json_strips_ansi_color(self) -> None:
        colored_empty = "\x1b[1;37m[\x1b[m\x1b[1;37m]\x1b[m\n"
        self.assertEqual(GATE.parse_json(colored_empty, "gh run list"), [])
        payload = [{"databaseId": 1, "headSha": "abc"}]
        colored = "".join(
            f"\x1b[1;37m{ch}\x1b[m" if ch in "[]{}" else ch for ch in json.dumps(payload)
        )
        self.assertEqual(GATE.parse_json(colored, "gh run list"), payload)
        with self.assertRaises(GATE.Unrun) as raised:
            GATE.parse_json("{not json", "gh run list")
        self.assertIn("invalid JSON", str(raised.exception))

    def test_command_timeout_refuses_tiny_leftover(self) -> None:
        deadlines = {"command": 60.0}
        with self.assertRaises(GATE.Unrun) as raised:
            GATE.command_timeout(deadlines, time.monotonic() + 0.05)
        self.assertIn("deadline reached", str(raised.exception))
        self.assertEqual(
            GATE.command_timeout({"command": 0.2}, time.monotonic() + 5),
            0.2,
        )
        leftover = GATE.command_timeout({"command": 60.0}, time.monotonic() + 2)
        self.assertGreater(leftover, 1.0)
        self.assertLessEqual(leftover, 2.0)

    def test_child_env_disables_forced_color(self) -> None:
        keys = ("CLICOLOR_FORCE", "FORCE_COLOR", "NO_COLOR", "GH_FORCE_TTY", "CLICOLOR")
        previous = {key: os.environ.get(key) for key in keys}
        try:
            os.environ["CLICOLOR_FORCE"] = "1"
            os.environ["FORCE_COLOR"] = "1"
            os.environ["GH_FORCE_TTY"] = "1"
            os.environ.pop("NO_COLOR", None)
            os.environ.pop("CLICOLOR", None)
            env = GATE.child_env()
            self.assertEqual(env.get("CLICOLOR_FORCE"), "0")
            self.assertEqual(env.get("FORCE_COLOR"), "0")
            self.assertEqual(env.get("CLICOLOR"), "0")
            self.assertEqual(env.get("NO_COLOR"), "1")
            self.assertNotIn("GH_FORCE_TTY", env)
        finally:
            for key, value in previous.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value

    def test_extracts_rust_and_swift_failure_names(self) -> None:
        rust = GATE.extract_rust_failures(
            "test native::foo::bar ... FAILED\n\nfailures:\n    native::foo::bar\n    native::foo::baz\n"
        )
        self.assertEqual(rust, ["native::foo::bar", "native::foo::baz"])
        swift = GATE.extract_swift_failures(
            "Test Case '-[BobMacCaptureTests.FooTests testBar]' failed (0.01 seconds).\n"
            '✘ Test "launch smoke" failed after 1 retry\n'
            "Test hangWatchdog() failed\n"
        )
        self.assertIn("BobMacCaptureTests.FooTests testBar", swift)
        self.assertIn("launch smoke", swift)
        self.assertIn("hangWatchdog", swift)

    def test_evaluate_run_pass_fail_and_missing_coverage(self) -> None:
        required = tuple(PROFILE["required_steps"])
        steps = [{"name": "Set up job", "status": "completed", "conclusion": "success"}]
        steps += [{"name": name, "status": "completed", "conclusion": "success"} for name in required]
        steps.append({"name": "Upload render fixtures", "status": "completed", "conclusion": "skipped"})
        job = {"name": PROFILE["job"], "status": "completed", "conclusion": "success", "steps": steps}
        payload = {
            "status": "completed", "conclusion": "success",
            "jobs": [{"name": "lint", "status": "completed", "conclusion": "success", "steps": []}, job],
        }
        code, _ = GATE.evaluate_run(payload, PROFILE)
        self.assertEqual(code, 0)

        failed_steps = [dict(step) for step in steps]
        failed_steps[1]["conclusion"] = "failure"
        fail_payload = {
            "status": "completed", "conclusion": "failure",
            "jobs": [{**job, "conclusion": "failure", "steps": failed_steps}],
        }
        self.assertEqual(GATE.evaluate_run(fail_payload, PROFILE)[0], 1)

        other = {
            "status": "completed", "conclusion": "failure",
            "jobs": [
                {"name": "lint", "status": "completed", "conclusion": "failure", "steps": []},
                job,
            ],
        }
        self.assertEqual(GATE.evaluate_run(other, PROFILE)[0], 1)

        missing_job = {"status": "completed", "conclusion": "success", "jobs": []}
        self.assertEqual(GATE.evaluate_run(missing_job, PROFILE)[0], 2)
        cancelled = {"status": "completed", "conclusion": "cancelled", "jobs": [job]}
        self.assertEqual(GATE.evaluate_run(cancelled, PROFILE)[0], 2)


class GateTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="macos-ci-check-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.checkout = self.root / "checkout"
        dest = self.checkout / REL_SCRIPT
        dest.parent.mkdir(parents=True)
        shutil.copyfile(SCRIPT_SRC, dest)
        dest.chmod(0o755)
        self.script = dest
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.stub("git", GIT_STUB)
        self.stub("gh", GH_STUB)
        (self.root / "git-state.json").write_text("{}")
        (self.root / "gh-state.json").write_text("{}")
        self.env = dict(os.environ)
        self.env.update(
            PATH=f"{self.bin}{os.pathsep}{self.env['PATH']}",
            TEST_ROOT=str(self.root),
            GATE_ROOT=str(self.checkout),
            GATE_REPO=REPO,
            GATE_JOB=PROFILE["job"],
            GATE_REQUIRED=json.dumps(list(PROFILE["required_steps"])),
            ORIGIN_URL=ORIGIN,
            HEAD_SHA=HEAD,
            SNAP_SHA=SNAP,
            TREE_SHA=TREE,
            RUN_ID=RUN_ID,
            GIT_MODE="",
            GH_MODE="success",
            BOB_MACOS_CI_DISCOVERY_SECONDS="2",
            BOB_MACOS_CI_TOTAL_SECONDS="6",
            BOB_MACOS_CI_COMMAND_SECONDS="3",
            BOB_MACOS_CI_CLEANUP_SECONDS="2",
            BOB_MACOS_CI_POLL_SECONDS="0.05",
        )

    def stub(self, name: str, body: str) -> None:
        path = self.bin / name
        path.write_text("#!/usr/bin/env python3\n" + body)
        path.chmod(0o755)

    def run_gate(self, **env: str) -> subprocess.CompletedProcess[str]:
        merged = {**self.env, **env}
        return subprocess.run(
            [sys.executable, "-B", str(self.script)],
            cwd=self.root,
            env=merged,
            capture_output=True,
            text=True,
        )

    def git_calls(self) -> list[dict]:
        return json_lines(self.root / "git.log")

    def gh_calls(self) -> list[list[str]]:
        return json_lines(self.root / "gh.log")

    def git_args(self) -> list[list[str]]:
        return [item["args"] for item in self.git_calls()]

    def assert_identity_filters(self, result: subprocess.CompletedProcess[str]) -> None:
        lists = [args for args in self.gh_calls() if args[:2] == ["run", "list"]]
        self.assertTrue(lists, result.stderr)
        args = lists[0]
        self.assertEqual(args[args.index("--repo") + 1], REPO)
        self.assertEqual(args[args.index("--workflow") + 1], "ci.yml")
        self.assertEqual(args[args.index("--event") + 1], "push")
        self.assertEqual(args[args.index("--commit") + 1], SNAP)
        branch = args[args.index("--branch") + 1]
        self.assertTrue(branch.startswith("ci/precommit/"), branch)
        views = [call for call in self.gh_calls() if call[:2] == ["run", "view"]]
        self.assertTrue(views, result.stderr)
        self.assertEqual(views[0][views[0].index("--repo") + 1], REPO)
        self.assertEqual(views[0][2], RUN_ID)

    def assert_no_local_mutation(self) -> None:
        forbidden = {"checkout", "switch", "stash", "reset", "update-ref", "branch", "commit"}
        for args in self.git_args():
            self.assertFalse(args and args[0] in forbidden, args)
            joined = args
            self.assertNotIn("--force", joined)
            dests = [a.split(":", 1)[-1] for a in args if ":" in a and not a.startswith("--")]
            self.assertNotIn("refs/heads/master", dests)
            self.assertNotIn("master", dests)

    def assert_cleanup_delete(self) -> None:
        deletes = [
            args for args in self.git_args()
            if args[:1] == ["push"] and any(a.startswith(":") for a in args)
        ]
        self.assertTrue(deletes)
        self.assertTrue(any(f"refs/heads/ci/precommit/" in a for a in deletes[0] for a in [a]))

    def test_help(self) -> None:
        help_result = subprocess.run(
            [sys.executable, "-B", str(self.script), "--help"],
            cwd=self.root, env=self.env, capture_output=True, text=True,
        )
        self.assertEqual(help_result.returncode, 0, help_result.stderr)
        text = help_result.stdout
        for needle in (
            "What is uploaded", "Prerequisites", "Result codes", "Cleanup",
            "python3 -B", REL_SCRIPT.as_posix(), "ci/precommit",
            "BOB_MACOS_CI_", "never pushes master",
        ):
            self.assertIn(needle, text)
        self.assertNotIn("token", text.lower())

    def test_help_short_flag(self) -> None:
        result = subprocess.run(
            [sys.executable, "-B", str(self.script), "-h"],
            cwd=self.root, env=self.env, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("exit codes" if False else "Result codes", result.stdout)

    def test_pass_and_cleanup(self) -> None:
        result = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(SNAP, result.stdout)
        self.assertIn(REPO, result.stdout)
        self.assertIn(PROFILE["job"], result.stdout)
        for name in PROFILE["required_steps"]:
            self.assertIn(name, result.stdout)
        self.assert_identity_filters(result)
        self.assert_no_local_mutation()
        self.assert_cleanup_delete()
        pushes = [args for args in self.git_args() if args[:1] == ["push"] and not any(a.startswith(":") for a in args)]
        self.assertTrue(any(a.startswith("--force-with-lease=") for a in pushes[0]))
        self.assertTrue(any(a.endswith(f":refs/heads/{a.split('ci/precommit/')[-1]}") or "ci/precommit/" in a for a in pushes[0]))

    def test_cwd_does_not_select_the_checkout(self) -> None:
        result = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stderr)
        tops = [item for item in self.git_calls() if item["args"][:2] == ["rev-parse", "--show-toplevel"]]
        self.assertEqual(Path(tops[0]["cwd"]).resolve(), self.script.parent.resolve())

    def test_logical_checkout_path_is_resolved(self) -> None:
        logical = self.root / "logical-checkout"
        logical.symlink_to(self.checkout)
        result = self.run_gate(GATE_ROOT=str(logical))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("is not inside git checkout", result.stderr)
        self.assert_cleanup_delete()

    def test_rust_or_swift_failure_prints_names(self) -> None:
        mode = "fail-rust" if PROFILE["failure_kind"] == "rust" else "fail-swift"
        result = self.run_gate(GH_MODE=mode)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        if mode == "fail-rust":
            self.assertIn("native::foo::bar", result.stdout)
        else:
            self.assertIn("BobMacCaptureTests.FooTests testBar", result.stdout)
            self.assertIn("launch smoke", result.stdout)
        self.assert_cleanup_delete()

    def test_lint_failure_keeps_later_skipped_steps_in_the_report(self) -> None:
        result = self.run_gate(GH_MODE="fail-lint-skip")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(PROFILE["required_steps"][0], result.stdout)
        self.assertIn("skipped", result.stdout)
        self.assertIn(PROFILE["required_steps"][-1], result.stdout)
        self.assert_cleanup_delete()

    def test_unrelated_job_failure(self) -> None:
        result = self.run_gate(GH_MODE="fail-other-job")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("lint", result.stdout)
        self.assert_cleanup_delete()

    def test_wrong_identity_is_ignored(self) -> None:
        result = self.run_gate(GH_MODE="wrong-identity")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("no matching", result.stderr)
        self.assertNotIn("timed out", result.stderr)
        self.assert_cleanup_delete()

    def test_mixed_identity_selects_the_matching_run(self) -> None:
        result = self.run_gate(GH_MODE="mixed-identity")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_identity_filters(result)

    def test_delayed_run_discovery(self) -> None:
        result = self.run_gate(GH_MODE="delayed", DELAY_COUNT="3", BOB_MACOS_CI_DISCOVERY_SECONDS="2")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        lists = [args for args in self.gh_calls() if args[:2] == ["run", "list"]]
        self.assertGreaterEqual(len(lists), 3)

    def test_colored_json_empty_then_matching_run(self) -> None:
        result = self.run_gate(
            GH_MODE="delayed",
            DELAY_COUNT="2",
            GH_COLOR_JSON="1",
            BOB_MACOS_CI_DISCOVERY_SECONDS="2",
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_cleanup_delete()

    def test_no_run(self) -> None:
        result = self.run_gate(GH_MODE="empty")
        self.assertEqual(result.returncode, 2)
        self.assertIn("no matching", result.stderr)
        self.assert_cleanup_delete()

    def test_queued_run_deadline_cancels(self) -> None:
        result = self.run_gate(
            GH_MODE="queued",
            BOB_MACOS_CI_TOTAL_SECONDS="4",
            BOB_MACOS_CI_DISCOVERY_SECONDS="2",
            BOB_MACOS_CI_COMMAND_SECONDS="1",
        )
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertTrue((self.root / "cancelled.json").exists(), result.stderr)
        self.assert_cleanup_delete()

    def test_cancelled_run(self) -> None:
        result = self.run_gate(GH_MODE="cancelled")
        self.assertEqual(result.returncode, 2)
        self.assertIn("cancelled", result.stderr + result.stdout)
        self.assert_cleanup_delete()

    def test_missing_required_job_and_step(self) -> None:
        missing_job = self.run_gate(GH_MODE="missing-job")
        self.assertEqual(missing_job.returncode, 2)
        missing_step = self.run_gate(GH_MODE="missing-step")
        self.assertEqual(missing_step.returncode, 2)

    def test_optional_skips_still_pass(self) -> None:
        result = self.run_gate(GH_MODE="skip-optional")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("skipped", result.stdout)

    def test_auth_error_does_not_push(self) -> None:
        result = self.run_gate(GH_MODE="auth")
        self.assertEqual(result.returncode, 2)
        self.assertFalse(any(args[:1] == ["push"] for args in self.git_args()))

    def test_push_error(self) -> None:
        result = self.run_gate(GIT_MODE="push-fail")
        self.assertEqual(result.returncode, 2)
        self.assert_cleanup_delete()

    def test_api_error_does_not_pass(self) -> None:
        result = self.run_gate(GH_MODE="api-error")
        self.assertEqual(result.returncode, 2)
        self.assert_cleanup_delete()

    def test_malformed_json_never_passes(self) -> None:
        result = self.run_gate(GH_MODE="malformed")
        self.assertEqual(result.returncode, 2)
        self.assertIn("invalid JSON", result.stderr)

    def test_stalled_subprocess(self) -> None:
        result = self.run_gate(GIT_MODE="stall", STALL_SECONDS="3", BOB_MACOS_CI_COMMAND_SECONDS="0.2")
        self.assertEqual(result.returncode, 2)
        self.assertIn("timed out", result.stderr)

    def test_known_failure_with_missing_logs(self) -> None:
        result = subprocess.run(
            [sys.executable, "-B", str(self.script)],
            cwd=self.root,
            env={**self.env, "GH_MODE": "fail-other-job"},
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 1)
        # Recreate stub behavior: fail-other-job still returns logs. Dedicated path:
        body = GH_STUB.replace(
            'if "--log-failed" in args:\n        if mode == "log-unavailable":',
            'if "--log-failed" in args:\n        if True or mode == "log-unavailable":',
        )
        self.stub("gh", body)
        result = self.run_gate(GH_MODE="fail-other-job")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("unavailable", result.stderr)

    def test_invalid_deadlines(self) -> None:
        for value in ("0", "-1", "inf", "nope"):
            with self.subTest(value=value):
                result = self.run_gate(BOB_MACOS_CI_TOTAL_SECONDS=value)
                self.assertEqual(result.returncode, 2)

    def test_merge_and_dirty_submodule_refused(self) -> None:
        merged = self.run_gate(GIT_MODE="merge")
        self.assertEqual(merged.returncode, 2)
        self.assertIn("merge", merged.stderr)
        status = "1 .M S.M. 100644 100644 100644 " + "a" * 40 + " " + "b" * 40 + " vendor/lib\n"
        dirty = self.run_gate(GIT_STATUS=status)
        self.assertEqual(dirty.returncode, 2)
        self.assertIn("submodule", dirty.stderr)

    def test_ambiguous_origin_rejected(self) -> None:
        result = self.run_gate(
            ORIGIN_URL="https://github.com/bobs-org/bob-cli.git\nhttps://github.com/bobs-org/bob-mac-capture.git\n"
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("multiple", result.stderr)

    def test_delete_retry_and_already_absent(self) -> None:
        retry = self.run_gate(GIT_MODE="delete-retry")
        self.assertEqual(retry.returncode, 0, retry.stderr)
        deletes = [
            args for args in self.git_args()
            if args[:1] == ["push"] and any(a.startswith(":") for a in args)
        ]
        self.assertGreaterEqual(len(deletes), 2)
        absent = self.run_gate(GIT_MODE="delete-absent")
        self.assertEqual(absent.returncode, 0, absent.stderr)

    def test_delete_failure_is_exit_2_not_pass(self) -> None:
        result = self.run_gate(GIT_MODE="delete-fail")
        self.assertEqual(result.returncode, 2)
        self.assertIn("recover", result.stderr)
        self.assertIn("git push origin :refs/heads/ci/precommit/", result.stderr)

    def test_delete_failure_after_ci_failure_is_still_2(self) -> None:
        result = self.run_gate(GIT_MODE="delete-fail", GH_MODE="fail-other-job")
        self.assertEqual(result.returncode, 2)
        self.assertIn("CI failed", result.stderr)

    def test_refuses_to_delete_another_sha(self) -> None:
        result = self.run_gate(GIT_MODE="delete-other-sha")
        self.assertEqual(result.returncode, 2)
        self.assertIn("refusing to delete", result.stderr)
        deletes = [
            args for args in self.git_args()
            if args[:1] == ["push"] and any(a.startswith(":") for a in args)
        ]
        self.assertEqual(deletes, [])

    def _wait_for(self, proc: subprocess.Popen[str], needle: str, timeout: float) -> str:
        deadline = time.monotonic() + timeout
        buf = ""
        while time.monotonic() < deadline:
            if proc.stdout is not None:
                line = proc.stdout.readline()
                buf += line
                if needle in buf:
                    return buf
            if proc.poll() is not None:
                rest = proc.stdout.read() if proc.stdout else ""
                buf += rest
                raise AssertionError(f"process exited before {needle!r}: {buf}")
        raise AssertionError(f"timed out waiting for {needle!r}: {buf}")

    def test_sigint_after_push(self) -> None:
        env = {**self.env, "GH_MODE": "stall", "STALL_SECONDS": "6", "BOB_MACOS_CI_COMMAND_SECONDS": "5"}
        proc = subprocess.Popen(
            [sys.executable, "-B", str(self.script)],
            cwd=self.root, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        output = self._wait_for(proc, "remote branch", 4)
        time.sleep(0.2)
        os.kill(proc.pid, signal.SIGINT)
        try:
            rest, _ = proc.communicate(timeout=6)
        except subprocess.TimeoutExpired:
            proc.kill()
            rest, _ = proc.communicate()
        output += rest
        self.assertEqual(proc.returncode, 2, output)
        self.assert_cleanup_delete()

    def test_sigterm_during_push(self) -> None:
        env = {
            **self.env,
            "GIT_MODE": "stall-push",
            "STALL_SECONDS": "6",
            "BOB_MACOS_CI_COMMAND_SECONDS": "5",
        }
        proc = subprocess.Popen(
            [sys.executable, "-B", str(self.script)],
            cwd=self.root, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        output = self._wait_for(proc, "pushing", 4)
        os.kill(proc.pid, signal.SIGTERM)
        try:
            rest, _ = proc.communicate(timeout=6)
        except subprocess.TimeoutExpired:
            proc.kill()
            rest, _ = proc.communicate()
        output += rest
        self.assertEqual(proc.returncode, 2, output)
        self.assert_cleanup_delete()


class GitFixtureTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="macos-ci-check-git-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.work = self.root / "work"
        self.bare = self.root / "remote.git"
        self.work.mkdir()
        self._git(self.work, ["init", "-b", "master"])
        self._git(self.work, ["config", "user.email", "test@example.com"])
        self._git(self.work, ["config", "user.name", "macos-ci-check"])
        self._git(self.work, ["config", "commit.gpgsign", "false"])
        (self.work / ".gitignore").write_text("ignored.txt\n")
        (self.work / "tracked.txt").write_text("head\n")
        (self.work / "delete me.txt").write_text("gone soon\n")
        (self.work / "file with spaces.txt").write_text("spaces-head\n")
        self._git(self.work, ["add", "-A"])
        self._git(self.work, ["commit", "-m", "init"])
        subprocess.run(["git", "init", "--bare", "-b", "master", str(self.bare)], check=True, capture_output=True)
        self._git(self.work, ["remote", "add", "origin", ORIGIN])
        self._git(self.work, ["config", f"url.{self.bare}.insteadOf", ORIGIN])
        self._git(self.work, ["push", "-u", "origin", "master"])
        dest = self.work / REL_SCRIPT
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(SCRIPT_SRC, dest)
        dest.chmod(0o755)
        self.script = dest
        self.bin = self.root / "bin"
        self.bin.mkdir()
        path = self.bin / "gh"
        path.write_text("#!/usr/bin/env python3\n" + GH_STUB)
        path.chmod(0o755)
        (self.root / "gh-state.json").write_text("{}")
        self.env = dict(os.environ)
        self.env.update(
            PATH=f"{self.bin}{os.pathsep}{self.env['PATH']}",
            TEST_ROOT=str(self.root),
            GATE_ROOT=str(self.work),
            GATE_REPO=REPO,
            GATE_JOB=PROFILE["job"],
            GATE_REQUIRED=json.dumps(list(PROFILE["required_steps"])),
            ORIGIN_URL=ORIGIN,
            GH_MODE="success",
            BOB_MACOS_CI_DISCOVERY_SECONDS="2",
            BOB_MACOS_CI_TOTAL_SECONDS="6",
            BOB_MACOS_CI_COMMAND_SECONDS="3",
            BOB_MACOS_CI_CLEANUP_SECONDS="2",
            BOB_MACOS_CI_POLL_SECONDS="0.05",
        )

    def _git(self, cwd: Path, args: list[str], **kwargs) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["git", "-C", str(cwd), *args], check=True, capture_output=True, text=True, **kwargs
        )

    def _snapshot_state(self) -> dict[str, object]:
        index = (self.work / ".git" / "index").read_bytes()
        head = self._git(self.work, ["rev-parse", "HEAD"]).stdout.strip()
        refs = self._git(self.work, ["show-ref"]).stdout
        files = {
            path.relative_to(self.work).as_posix(): path.read_bytes()
            for path in self.work.rglob("*")
            if path.is_file() and ".git" not in path.parts
        }
        return {"index": index, "head": head, "refs": refs, "files": files}

    def _prepare_dirty_tree(self) -> None:
        (self.work / "tracked.txt").write_text("staged\n")
        self._git(self.work, ["add", "tracked.txt"])
        (self.work / "tracked.txt").write_text("worktree\n")
        (self.work / "delete me.txt").unlink()
        (self.work / "file with spaces.txt").write_text("spaces-worktree\n")
        (self.work / "brand new.txt").write_text("untracked\n")
        (self.work / "ignored.txt").write_text("should not upload\n")

    def test_snapshot_uses_worktree_and_preserves_checkout(self) -> None:
        self._prepare_dirty_tree()
        before = self._snapshot_state()
        result = subprocess.run(
            [sys.executable, "-B", str(self.script)],
            cwd=self.root / "elsewhere" if False else self.root,
            env=self.env,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        after = self._snapshot_state()
        self.assertEqual(after, before)
        snap_line = next(line for line in result.stdout.splitlines() if "snapshot SHA" in line)
        sha = snap_line.rsplit(" ", 1)[-1]
        names = self._git(self.work, ["ls-tree", "-r", "--name-only", sha]).stdout.splitlines()
        self.assertIn("tracked.txt", names)
        self.assertIn("brand new.txt", names)
        self.assertIn("file with spaces.txt", names)
        self.assertNotIn("delete me.txt", names)
        self.assertNotIn("ignored.txt", names)
        self.assertEqual(self._git(self.work, ["show", f"{sha}:tracked.txt"]).stdout, "worktree\n")
        self.assertEqual(self._git(self.work, ["show", f"{sha}:brand new.txt"]).stdout, "untracked\n")
        self.assertEqual(
            self._git(self.work, ["show", f"{sha}:file with spaces.txt"]).stdout, "spaces-worktree\n"
        )
        refs = self._git(self.work, ["for-each-ref", "--format=%(refname)", "refs/heads/ci"]).stdout.strip()
        self.assertEqual(refs, "")
        remote = subprocess.run(
            ["git", "--git-dir", str(self.bare), "for-each-ref", "refs/heads/ci"],
            capture_output=True, text=True, check=True,
        )
        self.assertEqual(remote.stdout.strip(), "")

    def test_failed_gate_still_preserves_checkout(self) -> None:
        self._prepare_dirty_tree()
        before = self._snapshot_state()
        result = subprocess.run(
            [sys.executable, "-B", str(self.script)],
            cwd=self.work,
            env={**self.env, "GH_MODE": "fail-other-job"},
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(self._snapshot_state(), before)


if __name__ == "__main__":
    unittest.main()
