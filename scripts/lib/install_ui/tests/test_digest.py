from __future__ import annotations

"""Tests for digest levels, budget, layout, plugins, and records."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from install_ui import commits, digest, term

try:
    from install_ui.tests.test_commits import TempRepo, _commit, _git
except ImportError:  # pragma: no cover - direct-file runs
    from test_commits import TempRepo, _commit, _git  # type: ignore[no-redef]

LIB = Path(__file__).resolve().parents[1]


def _made(
    sha: str,
    subject: str,
    body: str = "",
    scope_check: bool = False,  # noqa: ARG001 - kept for call-site clarity
) -> commits.CommitInfo:
    return commits.parse_commit(sha, subject, body)


def _repo(
    name: str,
    made_commits: list[commits.CommitInfo],
    label: str = "3 commits since your last install 51m ago",
    kind: str = "watermark",
    directory: str = "/tmp/demo",
    base: str = "a" * 40,
    head: str = "b" * 40,
) -> digest.RenderRepo:
    return digest.RenderRepo(
        name=name,
        directory=directory,
        color_code="34",
        head=head or (made_commits[0].sha if made_commits else "b" * 40),
        base=base,
        label=label,
        commit_list=made_commits,
        kind=kind,
    )


def _widths(rendered: list[str]) -> list[int]:
    return [term.cell_width(term.strip_ansi(line)) for line in rendered]


class BannerTests(unittest.TestCase):
    def test_matches_term_banner_at_full_width(self) -> None:
        self.assertEqual(
            digest._banner_lines(79, False),
            term.banner(1, "📰", "WHAT'S NEW", colorize=False).splitlines(),
        )
        self.assertEqual(
            digest._banner_lines(79, True),
            term.banner(1, "📰", "WHAT'S NEW", colorize=True).splitlines(),
        )

    def test_rule_shrinks_on_narrow_panes(self) -> None:
        for line in digest._banner_lines(39, False):
            self.assertLessEqual(term.cell_width(line), 39)


class BudgetTests(unittest.TestCase):
    def test_formula(self) -> None:
        # reserve = 18 + 5*steps; budget = clamp(LINES - reserve, 8, 0.6*LINES)
        self.assertEqual(digest.budget(79, 3), 79 - 33)
        self.assertEqual(digest.budget(24, 3), 8)
        self.assertEqual(digest.budget(200, 3), (200 * 6) // 10)

    def test_measure_caps_at_100(self) -> None:
        self.assertEqual(digest.measure(260), 100)
        self.assertEqual(digest.measure(80), 79)


class LevelTests(unittest.TestCase):
    def _three(self) -> list[commits.CommitInfo]:
        return [
            _made(
                "8" * 40,
                "feat(gmail): add durable engine",
                "Implements versioned frontmatter.\n\nVerified: cargo test clean.\n",
            ),
            _made("6" * 40, "fix(capture): repair landing behavior", ""),
            _made(
                "7" * 40,
                "feat(capture)!: glue start timing",
                "BREAKING CHANGE: the old flow is gone.\n",
            ),
        ]

    def test_richest_level_that_fits_wins(self) -> None:
        repos = [_repo("bob-cli", self._three())]
        text, record = digest.render_section(
            repos, cols=80, lines=79, steps=3, colorize=False, tty=True
        )
        self.assertEqual(record["level"], "full")
        self.assertIn("Verified:", text)
        self.assertIn("…", text)  # folded verified paragraph

    def test_tiny_screen_forces_grouped(self) -> None:
        made = [
            _made(f"{i:040d}", f"feat(gmail): change number {i}", "Body text.\n")
            for i in range(12)
        ]
        repos = [_repo("bob-cli", made, label="12 commits in this pull")]
        text, record = digest.render_section(
            repos, cols=80, lines=24, steps=3, colorize=False, tty=True
        )
        self.assertEqual(record["level"], "grouped")
        self.assertIn("+ ", text)
        self.assertIn("more ·", text)
        self.assertIn("git -C ", text)
        self.assertIn("log --oneline", text)

    def test_breaking_never_folds_into_more(self) -> None:
        made = [_made("7" * 40, "feat(x)!: rewrite", "BREAKING CHANGE: gone.\n")]
        made += [
            _made(f"{i:040d}", f"feat(gmail): change number {i}", "")
            for i in range(30)
        ]
        repos = [_repo("bob-cli", made, label="31 commits in this pull")]
        text, record = digest.render_section(
            repos, cols=80, lines=24, steps=3, colorize=False, tty=True
        )
        self.assertEqual(record["level"], "grouped")
        self.assertIn("BREAKING CHANGE: gone.", text)

    def test_volumes(self) -> None:
        for count in (0, 1, 3, 16, 59):
            made = [
                _made(f"{i:040d}", f"fix(area): change number {i}", "")
                for i in range(count)
            ]
            repos = [_repo("bob-cli", made, label=f"{count} commits in this pull")]
            text, record = digest.render_section(
                repos, cols=80, lines=79, steps=3, colorize=False, tty=True
            )
            if count == 0:
                self.assertEqual(text, "")
            else:
                self.assertIn("bob-cli", text)
            self.assertEqual(record["repos"][0]["count"] if record["repos"] else 0, count)


class LayoutTests(unittest.TestCase):
    def _three(self) -> list[commits.CommitInfo]:
        return [
            _made("8" * 40, "feat(gmail): add durable engine", "Body.\n"),
            _made("6" * 40, "fix(capture): repair landing", ""),
            _made("0" * 40, "docs(notes): polish wording", ""),
        ]

    def test_rows_fit_measure_across_sizes(self) -> None:
        for cols, lines in (
            (40, 24),
            (40, 40),
            (40, 79),
            (80, 24),
            (80, 40),
            (80, 79),
            (260, 24),
            (260, 40),
            (260, 79),
        ):
            repos = [_repo("bob-cli", self._three())]
            text, _ = digest.render_section(
                repos, cols=cols, lines=lines, steps=3,
                colorize=False, tty=True,
            )
            width = digest.measure(cols)
            for line in text.splitlines():
                self.assertLessEqual(
                    term.cell_width(line), width, f"cols={cols} lines={lines}: {line!r}"
                )

    def test_narrow_drops_sha(self) -> None:
        repos = [_repo("bob-cli", self._three())]
        text, _ = digest.render_section(
            repos, cols=50, lines=79, steps=3, colorize=False, tty=True
        )
        self.assertNotIn("88888888", text)

    def test_very_narrow_folds_scope(self) -> None:
        repos = [_repo("bob-cli", self._three())]
        text, _ = digest.render_section(
            repos, cols=40, lines=79, steps=3, colorize=False, tty=True
        )
        self.assertIn("gmail:", text)

    def test_hostile_content_is_inert(self) -> None:
        evil = _made(
            "1" * 40,
            "feat(x): hello\x1b[2Jworld\x07",
            "Body with \x1b[31m color and wide chars: \u6f22\u5b57.\n"
            "Second line\rcarriage return.\n",
        )
        repos = [_repo("bob-cli", [evil])]
        text, _ = digest.render_section(
            repos, cols=80, lines=79, steps=3, colorize=False, tty=True
        )
        self.assertNotIn("\x1b[2J", text)
        self.assertNotIn("\x07", text)
        self.assertNotIn("\r", text)
        self.assertIn("^M", text)
        self.assertNotIn("\x1b[31m", text)
        for line in text.splitlines():
            self.assertLessEqual(term.cell_width(line), digest.measure(80))

    def test_breaking_badge_and_migration_note(self) -> None:
        made = [_made("7" * 40, "feat(x)!: rewrite everything", "")]
        made += [_made(f"{i:040d}", f"fix(area): routine fix {i}", "") for i in range(10)]
        repos = [_repo("bob-cli", made, label="11 commits in this pull")]
        text, record = digest.render_section(
            repos, cols=80, lines=24, steps=3, colorize=False, tty=True
        )
        self.assertEqual(record["level"], "grouped")
        self.assertIn("⚠", text)
        self.assertIn("the commit has no migration note", text)

    def test_colored_snapshot_has_ansi(self) -> None:
        repos = [_repo("bob-cli", self._three())]
        text, _ = digest.render_section(
            repos, cols=80, lines=79, steps=3, colorize=True, tty=True
        )
        self.assertIn("\033[", text)
        self.assertIn("\033[32m", text)  # feat green
        self.assertIn("\n  \033[1;34mbob-cli", text)
        self.assertIn("\n    \033[32mfeat", text)


class GoldenTests(unittest.TestCase):
    def _screen_a_commits(self) -> list[commits.CommitInfo]:
        return [
            _made(
                "79709fed" + "0" * 32,
                "feat(capture): glue start and past timing to Pomodoro names",
                "",
            ),
            _made(
                "8935148f" + "0" * 32,
                "feat(gmail): add durable send-recovery engine for compose drafts",
                "Implements versioned frontmatter and recovery.\n"
                "Internal only, no public send command.\n"
                "\nVerified: cargo fmt --check clean, workspace clippy gate.\n",
            ),
            _made(
                "86b2501b" + "0" * 32,
                "fix(capture): repair successor verdict landing behavior",
                "",
            ),
        ]

    def test_screen_a_digest_part_golden(self) -> None:
        repos = [
            _repo(
                "bob-cli",
                self._screen_a_commits(),
                label="3 commits since your last install 51m ago",
            )
        ]
        text, record = digest.render_section(
            repos, cols=80, lines=79, steps=3, colorize=False, tty=True
        )
        self.assertEqual(record["level"], "full")
        self.assertIn("📰  WHAT'S NEW", text)
        self.assertIn("bob-cli · 3 commits since your last install 51m ago", text)
        self.assertIn("79709fed", text)
        self.assertIn("8935148f", text)
        self.assertIn("86b2501b", text)
        self.assertIn("Verified: cargo fmt --check clean", text)
        self.assertIn("…", text)
        # Golden: the exact plain-text section for this fixed input.
        self.assertEqual(
            text,
            "\n".join(
                [
                    "",
                    "📰  WHAT'S NEW",
                    "────────────────────────────────────────────────",
                    "  bob-cli · 3 commits since your last install 51m ago",
                    "    feat  capture  glue start and past timing to Pomodoro names        79709fed",
                    "    feat  gmail    add durable send-recovery engine for compose drafts 8935148f",
                    "                   Implements versioned frontmatter and recovery.",
                    "                   Internal only, no public send command.",
                    "                   Verified: cargo fmt --check clean, workspace clippy gate.…",
                    "",
                    "    fix   capture  repair successor verdict landing behavior           86b2501b",
                ]
            )
            + "\n",
        )

    def test_screen_e_grouped_golden_shape(self) -> None:
        made = [
            _made("0869f792" + "0" * 32, "feat(capture): rewrite relation directions", ""),
            _made("79709fed" + "0" * 32, "feat(capture): glue start timing", ""),
            _made("8935148f" + "0" * 32, "feat(gmail): add durable engine", ""),
            _made("b6a96243" + "0" * 32, "feat(gmail): add safe draft flow", ""),
            _made("86b2501b" + "0" * 32, "fix(capture): repair landing", ""),
        ]
        made += [
            _made(f"{i:040d}", "feat(gmail): routine change", "") for i in range(19)
        ]
        made += [_made(f"9{i:039d}", "fix(capture): routine fix", "") for i in range(6)]
        made += [_made(f"8{i:039d}", "docs(notes): routine docs", "") for i in range(5)]
        repos = [
            _repo("bob-cli", made, label="37 commits since your last install yesterday 23:31")
        ]
        text, record = digest.render_section(
            repos, cols=80, lines=24, steps=3, colorize=False, tty=True
        )
        self.assertEqual(record["level"], "grouped")
        self.assertIn("+", text)
        self.assertIn("more ·", text)
        self.assertIn("git -C ", text)
        self.assertIn("\n  bob-cli ·", text)
        self.assertIn("\n    feat", text)
        self.assertIn("\n    + ", text)
        self.assertIn("\n      git -C ", text)


class PluginTests(unittest.TestCase):
    def _fake_bob(self, directory: str, payload: str, code: int = 0) -> str:
        path = os.path.join(directory, "bob")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(f"#!/bin/sh\ncat <<'EOF'\n{payload}\nEOF\nexit {code}\n")
        os.chmod(path, 0o755)
        return path

    def test_lists_pending_plugins(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            payload = json.dumps(
                {
                    "plugins": [
                        {"id": "a", "files": [{"name": "main.js", "action": "updated"}]},
                        {"id": "b", "files": [{"name": "main.js", "action": "unchanged"}]},
                        {"id": "c", "files": [{"name": "main.js", "action": "created"}]},
                    ]
                }
            )
            bob = self._fake_bob(tmp, payload)
            self.assertEqual(digest.plugin_updates(bob, tmp), ["a", "c"])

    def test_bad_json_is_none(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bob = self._fake_bob(tmp, "not json{{")
            self.assertIsNone(digest.plugin_updates(bob, tmp))

    def test_timeout_is_none(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "bob")
            with open(path, "w", encoding="utf-8") as handle:
                handle.write("#!/bin/sh\nsleep 30\n")
            os.chmod(path, 0o755)
            self.assertIsNone(digest.plugin_updates(path, tmp, timeout=0.2))


class NonTtyTests(unittest.TestCase):
    def test_cap_at_60_with_more_line(self) -> None:
        made = [
            _made(f"{i:040d}", f"fix(area): change number {i}", "Body.\n")
            for i in range(70)
        ]
        repos = [_repo("bob-cli", made, label="70 commits in this pull")]
        text, record = digest.render_section(
            repos, cols=80, lines=79, steps=3, colorize=False, tty=False
        )
        self.assertNotIn("\033[", text)
        self.assertIn("+ 10 more", text)
        self.assertEqual(record["level"], "subjects")
        subject_rows = [
            line for line in text.splitlines()
            if line.startswith("    ") and not line.startswith("    + ")
        ]
        self.assertEqual(len(subject_rows), 60)
        more_rows = [line for line in text.splitlines() if "+ 10 more" in line]
        self.assertEqual(len(more_rows), 1)


class RecordTests(unittest.TestCase):
    def test_record_shape(self) -> None:
        made = [
            _made("8" * 40, "feat(gmail): engine", "Body.\n"),
            _made("7" * 40, "feat(x)!: breaking", "BREAKING CHANGE: gone.\n"),
        ]
        repos = [_repo("bob-cli", made)]
        _, record = digest.render_section(
            repos, cols=80, lines=79, steps=3, colorize=False, tty=True
        )
        self.assertEqual(record["v"], 1)
        self.assertIn(record["level"], ("full", "lead", "subjects", "grouped"))
        entry = record["repos"][0]
        self.assertEqual(entry["breaking"], 1)
        self.assertEqual(len(entry["subjects"]), 2)
        self.assertTrue(entry["head"].startswith("8" * 8) or len(entry["head"]) == 40)


class EndToEndTests(unittest.TestCase):
    def test_digest_subcommand_against_real_repo(self) -> None:
        tmp = TempRepo()
        self.addCleanup(tmp.cleanup)
        first = _commit(tmp.repo, "a.txt", "feat(a): first")
        _commit(tmp.repo, "b.txt", "fix(a): second change")
        from install_ui import state as state_mod

        state_mod.write_watermark(
            "bob-cli", {"sha": first, "dirty": False, "path": tmp.repo}
        )
        record_path = str(Path(tmp.tmp.name) / "digest.json")
        env = dict(os.environ)
        env["BOB_INSTALL_STATE_DIR"] = tmp.state
        env["COLUMNS"] = "80"
        env["LINES"] = "79"
        proc = subprocess.run(
            [sys.executable, "-B", str(LIB), "digest",
             "--repo", f"bob-cli={tmp.repo}", "--record", record_path],
            capture_output=True,
            text=True,
            env=env,
            timeout=60,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("WHAT'S NEW", proc.stdout)
        self.assertIn("fix(a): second change", proc.stdout)
        with open(record_path, encoding="utf-8") as handle:
            record = json.load(handle)
        self.assertEqual(record["v"], 1)
        self.assertEqual(record["repos"][0]["count"], 1)

    def test_nothing_new_prints_nothing(self) -> None:
        tmp = TempRepo()
        self.addCleanup(tmp.cleanup)
        head = _commit(tmp.repo, "a.txt", "feat(a): first")
        from install_ui import state as state_mod

        state_mod.write_watermark(
            "bob-cli", {"sha": head, "dirty": False, "path": tmp.repo}
        )
        env = dict(os.environ)
        env["BOB_INSTALL_STATE_DIR"] = tmp.state
        proc = subprocess.run(
            [sys.executable, "-B", str(LIB), "digest",
             "--repo", f"bob-cli={tmp.repo}"],
            capture_output=True,
            text=True,
            env=env,
            timeout=60,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stdout, "")

    def test_bad_repo_exits_1(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            state_dir = str(Path(tmp) / "state")
            os.makedirs(state_dir)
            env = dict(os.environ)
            env["BOB_INSTALL_STATE_DIR"] = state_dir
            proc = subprocess.run(
                [sys.executable, "-B", str(LIB), "digest",
                 "--repo", "bob-cli=/no/such/dir"],
                capture_output=True,
                text=True,
                env=env,
                timeout=60,
            )
            self.assertEqual(proc.returncode, 1)


class GitBackedRenderTests(unittest.TestCase):
    def test_git_repo_renders_with_sase_trailers(self) -> None:
        tmp = TempRepo()
        self.addCleanup(tmp.cleanup)
        first = _commit(tmp.repo, "a.txt", "feat(a): first")
        _commit(
            tmp.repo,
            "b.txt",
            "feat(gmail): add engine\n\nReal body.\n\nSASE_FOO=1\n[1]: https://example.com\n",
        )
        spec = f"{first}..{_git(tmp.repo, 'rev-parse', 'HEAD').strip()}"
        found = commits.list_commits(tmp.repo, spec)
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].stripped_body, "Real body.")
        repos = [_repo("bob-cli", found)]
        text, _ = digest.render_section(
            repos, cols=80, lines=79, steps=3, colorize=False, tty=True
        )
        self.assertNotIn("SASE_FOO", text)
        self.assertIn("Real body.", text)


if __name__ == "__main__":
    unittest.main()
