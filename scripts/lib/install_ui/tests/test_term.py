from __future__ import annotations

import io
import os
import socket
import subprocess
import unittest
from datetime import datetime, timedelta, timezone
from unittest import mock

from install_ui import term


class UseColorTests(unittest.TestCase):
    def test_tty_without_no_color(self) -> None:
        stream = mock.Mock()
        stream.isatty.return_value = True
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("NO_COLOR", None)
            os.environ.pop("TERM", None)
            self.assertTrue(term.use_color(stream))

    def test_no_color_disables(self) -> None:
        stream = mock.Mock()
        stream.isatty.return_value = True
        with mock.patch.dict(os.environ, {"NO_COLOR": ""}):
            self.assertFalse(term.use_color(stream))

    def test_dumb_term_disables(self) -> None:
        stream = mock.Mock()
        stream.isatty.return_value = True
        env = {"TERM": "dumb"}
        with mock.patch.dict(os.environ, env, clear=False):
            os.environ.pop("NO_COLOR", None)
            self.assertFalse(term.use_color(stream))

    def test_non_tty_disables(self) -> None:
        stream = io.StringIO()
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("NO_COLOR", None)
            os.environ.pop("TERM", None)
            self.assertFalse(term.use_color(stream))


class BannerTests(unittest.TestCase):
    def test_color_matches_install_all_fixture(self) -> None:
        expected = (
            "\n🧩  \033[1;34mBOB-PLUGINS  2/3\033[0m\n"
            "\033[34m────────────────────────────────────────────────\033[0m\n"
        )
        self.assertEqual(
            term.banner(34, "🧩", "BOB-PLUGINS  2/3", colorize=True),
            expected,
        )

    def test_plain_matches_install_all_fixture(self) -> None:
        expected = (
            "\n🧩  BOB-PLUGINS  2/3\n"
            "────────────────────────────────────────────────\n"
        )
        self.assertEqual(
            term.banner(34, "🧩", "BOB-PLUGINS  2/3", colorize=False),
            expected,
        )

    def test_summary_bold_color_one(self) -> None:
        expected = (
            "\n✨  \033[1;1mSUMMARY\033[0m\n"
            "\033[1m────────────────────────────────────────────────\033[0m\n"
        )
        self.assertEqual(term.banner(1, "✨", "SUMMARY", colorize=True), expected)

    def test_matches_bash_printf(self) -> None:
        script = r"""
rule='────────────────────────────────────────────────'
color=$1 icon=$2 title=$3 colorize=$4
if [[ "$colorize" == 1 ]]; then
  printf '\n%s  \033[1;%sm%s\033[0m\n\033[%sm%s\033[0m\n' \
    "$icon" "$color" "$title" "$color" "$rule"
else
  printf '\n%s  %s\n%s\n' "$icon" "$title" "$rule"
fi
"""
        for colorize in (True, False):
            bash = subprocess.check_output(
                ["bash", "-c", script, "banner", "35", "🦀", "BOB-CLI  1/3",
                 "1" if colorize else "0"],
            )
            py = term.banner(35, "🦀", "BOB-CLI  1/3", colorize=colorize)
            self.assertEqual(py.encode("utf-8"), bash)

    def test_paint_color_and_plain(self) -> None:
        self.assertEqual(term.paint(32, "ok", colorize=True), "\033[32mok\033[0m")
        self.assertEqual(term.paint(32, "ok", colorize=False), "ok")


class CellWidthTests(unittest.TestCase):
    def test_ascii_fullwidth_combining(self) -> None:
        self.assertEqual(term.cell_width("abc"), 3)
        self.assertEqual(term.cell_width("你好"), 4)
        self.assertEqual(term.cell_width("a\u0301"), 1)

    def test_truncate_appends_ellipsis(self) -> None:
        self.assertEqual(term.truncate_cells("hello world", 8), "hello w…")
        self.assertEqual(term.truncate_cells("hello", 8), "hello")
        self.assertEqual(term.truncate_cells("你好世界", 5), "你好…")

    def test_wrap_breaks_on_spaces_and_long_words(self) -> None:
        self.assertEqual(
            term.wrap_cells("one two three four", 10),
            ["one two", "three four"],
        )
        wrapped = term.wrap_cells("abcdefghij", 4)
        self.assertEqual(wrapped, ["abcd", "efgh", "ij"])
        self.assertEqual(term.wrap_cells("", 10), [""])

    def test_strip_ansi(self) -> None:
        self.assertEqual(term.strip_ansi("\033[1;31mred\033[0m"), "red")
        self.assertEqual(term.cell_width("\033[32mhi\033[0m"), 2)


class SanitizeTests(unittest.TestCase):
    def test_esc_tab_del_newline_c1(self) -> None:
        self.assertEqual(term.sanitize("\033"), "^[")
        self.assertEqual(term.sanitize("a\tb"), "a b")
        self.assertEqual(term.sanitize("\x7f"), "^?")
        self.assertEqual(term.sanitize("a\nb"), "a^Jb")
        self.assertEqual(term.sanitize("\x9b"), "^[")
        self.assertEqual(term.sanitize("ok"), "ok")


class TimeFormatTests(unittest.TestCase):
    def test_duration_matches_bash(self) -> None:
        script = r"""
format_duration() {
  local seconds="$1"
  if (( seconds < 60 )); then
    printf '%ss' "$seconds"
  else
    printf '%dm %02ds' "$((seconds / 60))" "$((seconds % 60))"
  fi
}
format_duration "$1"
"""
        for secs in (0, 1, 45, 59, 60, 72, 492, 5400):
            bash = subprocess.check_output(
                ["bash", "-c", script, "dur", str(secs)],
                text=True,
            )
            self.assertEqual(term.format_duration(secs), bash)

    def test_clock(self) -> None:
        self.assertEqual(term.format_clock(3), "0:03")
        self.assertEqual(term.format_clock(292), "4:52")
        self.assertEqual(term.format_clock(3723), "1:02:03")

    def test_relative(self) -> None:
        now = datetime(2026, 11, 6, 14, 16, tzinfo=timezone(timedelta(hours=-5)))
        self.assertEqual(
            term.format_relative(now - timedelta(seconds=12), now),
            "just now",
        )
        self.assertEqual(
            term.format_relative(now - timedelta(minutes=51), now),
            "51m ago",
        )
        self.assertEqual(
            term.format_relative(now - timedelta(hours=3), now),
            "3h ago",
        )
        yesterday = datetime(2026, 11, 5, 23, 31, tzinfo=now.tzinfo)
        self.assertEqual(
            term.format_relative(yesterday, now),
            "yesterday 23:31",
        )
        self.assertEqual(
            term.format_relative(datetime(2026, 11, 2, 9, 0, tzinfo=now.tzinfo), now),
            "Nov 2",
        )
        self.assertEqual(
            term.format_relative(datetime(2025, 11, 2, 9, 0, tzinfo=now.tzinfo), now),
            "Nov 2 2025",
        )


class TerminalSizeAndHostTests(unittest.TestCase):
    def test_falls_back_to_env_then_default(self) -> None:
        stream = mock.Mock()
        stream.fileno.side_effect = OSError("no tty")
        with mock.patch.dict(os.environ, {"COLUMNS": "100", "LINES": "40"}):
            self.assertEqual(term.terminal_size(stream), (100, 40))
        with mock.patch.dict(os.environ, {"COLUMNS": "nope"}, clear=False):
            os.environ.pop("LINES", None)
            stream.fileno.side_effect = OSError("no tty")
            self.assertEqual(term.terminal_size(stream), (80, 24))

    def test_prefers_stream_size(self) -> None:
        stream = mock.Mock()
        stream.fileno.return_value = 1
        size = os.terminal_size((90, 30))
        with mock.patch("os.get_terminal_size", return_value=size):
            self.assertEqual(term.terminal_size(stream), (90, 30))

    def test_host_label(self) -> None:
        with mock.patch("sys.platform", "darwin"):
            self.assertEqual(term.host_label(), "this Mac")
        with mock.patch("sys.platform", "linux"):
            with mock.patch.object(socket, "gethostname", return_value="athena.lan"):
                self.assertEqual(term.host_label(), "athena")


if __name__ == "__main__":
    unittest.main()
