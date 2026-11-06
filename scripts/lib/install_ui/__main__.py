from __future__ import annotations

import argparse
import json
import sys
import traceback
from pathlib import Path
from typing import Optional, Sequence

_LIB = Path(__file__).resolve().parent.parent
_LIB_S = str(_LIB)
if sys.path[:1] != [_LIB_S]:
    if _LIB_S in sys.path:
        sys.path.remove(_LIB_S)
    sys.path.insert(0, _LIB_S)

from install_ui import commits, digest, history, live, state, term  # noqa: E402


def _warn(message: str) -> None:
    painted = term.paint("2", message, stream=sys.stderr)
    sys.stderr.write(painted + "\n")


def _crash() -> int:
    result = state.append_crash(traceback.format_exc())
    if result.ok:
        loc = str(result.value)
    else:
        loc = str(state.state_dir() / "crash.log")
    _warn(f"install_ui: internal error (details in {loc})")
    return 70


def cmd_mark_installed(args: argparse.Namespace) -> int:
    try:
        snap = state.snapshot_repo(args.repo_dir)
        if not snap.ok:
            _warn(f"install_ui: {snap.error}")
            return 0
        written = state.write_watermark(args.repo, snap.value)
        if not written.ok:
            _warn(f"install_ui: {written.error}")
        return 0
    except Exception:
        state.append_crash(traceback.format_exc())
        _warn("install_ui: failed to record watermark")
        return 0


def cmd_watermark(args: argparse.Namespace) -> int:
    result = state.get_watermark(args.repo)
    if not result.ok:
        _warn(f"install_ui: {result.error}")
        return 1
    record = result.value
    if args.json:
        json.dump(record, sys.stdout, indent=2, sort_keys=True)
        sys.stdout.write("\n")
        return 0
    dirty = "dirty" if record.get("dirty") else "clean"
    sys.stdout.write(
        f"{args.repo}  {record.get('sha', '')}  {record.get('at', '')}  "
        f"{dirty}  {record.get('path', '')}\n"
    )
    return 0


def _parse_name_value(values: Optional[list], label: str) -> dict:
    parsed: dict = {}
    for item in values or []:
        name, sep, value = item.partition("=")
        if not sep or not name or not value:
            raise ValueError(f"--{label} must be NAME=VALUE, got {item!r}")
        parsed[name] = value
    return parsed


def cmd_digest(args: argparse.Namespace) -> int:
    try:
        repos = _parse_name_value(args.repo, "repo")
        pres = _parse_name_value(args.pre, "pre")
        posts = _parse_name_value(args.post, "post")
        pulls = _parse_name_value(args.pull, "pull")
        colors = _parse_name_value(args.color, "color")
    except ValueError as exc:
        _warn(f"install_ui: {exc}")
        return 2
    valid_pull = {"updated", "current", "warn", "cloned", "none"}
    for name, value in pulls.items():
        if value not in valid_pull:
            _warn(f"install_ui: --pull state for {name} must be one of updated|current|warn|cloned|none")
            return 2
    if not repos:
        _warn("install_ui: digest needs at least one --repo NAME=DIR")
        return 2
    if args.steps < 1:
        _warn("install_ui: --steps must be at least 1")
        return 2
    render_repos: list = []
    usable = 0
    for name, directory in repos.items():
        pull_state = pulls.get(name, "none")
        outcome = commits.resolve_range(
            name, directory, pull_state, pres.get(name, ""), posts.get(name, ""),
        )
        if outcome.invalid:
            continue
        usable += 1
        if outcome.skip:
            continue
        spec = f"{outcome.base}..{outcome.head}" if outcome.base else ""
        found = commits.list_commits(directory, spec) if spec else []
        notes: list = []
        if outcome.note:
            notes.append(outcome.note)
        if outcome.dirty_files:
            noun = "file" if outcome.dirty_files == 1 else "files"
            notes.append(
                f"installing with uncommitted changes in {outcome.dirty_files} {noun}"
            )
        render_repos.append(
            digest.RenderRepo(
                name=name,
                directory=directory,
                color_code=colors.get(name, "1"),
                head=outcome.head,
                base=outcome.base,
                label=outcome.label,
                notes=notes,
                commit_list=found,
                kind=outcome.kind,
            )
        )
    if usable == 0:
        _warn("install_ui digest: no usable repos")
        return 1
    plugin_ids: dict = {}
    if args.plugins:
        for name, directory in repos.items():
            if name == args.plugins and args.bob_bin:
                pending = digest.plugin_updates(args.bob_bin, directory)
                if pending:
                    plugin_ids[name] = pending
    try:
        cols, lines = term.terminal_size(sys.stdout)
    except Exception:
        cols, lines = 80, 24
    colorize = term.use_color(sys.stdout)
    try:
        tty = bool(sys.stdout.isatty())
    except Exception:
        tty = False
    text, record = digest.render_section(
        render_repos, cols, lines, args.steps, colorize, tty, plugin_ids
    )
    if args.record:
        if not digest.write_record(args.record, record):
            _warn("install_ui digest: could not write --record file")
    if text:
        sys.stdout.write(text)
    return 0


def cmd_record(args: argparse.Namespace) -> int:
    result = history.record(
        step=args.step,
        phase=args.phase,
        secs=args.secs,
        outcome=args.outcome,
        kind=args.kind,
        waited=args.waited,
        profile=args.profile,
        toolchain=args.toolchain,
        incremental=None if args.incremental == "unknown" else args.incremental == "on",
    )
    if not result.ok:
        _warn(f"install_ui: {result.error}")
        return 1
    return 0


def cmd_kind(args: argparse.Namespace) -> int:
    kind = history.predict_kind(args.step, args.repo, args.profile)
    sys.stdout.write(kind + "\n")
    return 0


def cmd_run(args: argparse.Namespace) -> int:
    return live.run(args)


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="install_ui",
        description="Terminal UI helper for just install and just install-all.",
    )
    sub = parser.add_subparsers(dest="command", metavar="command")
    sub.required = True

    dg = sub.add_parser(
        "digest",
        help="Print the What's new digest section.",
    )
    dg.add_argument("--bob-bin", default="", metavar="PATH")
    dg.add_argument("--color", action="append", default=[], metavar="NAME=ANSI")
    dg.add_argument("--plugins", default="", metavar="NAME")
    dg.add_argument("--post", action="append", default=[], metavar="NAME=SHA")
    dg.add_argument("--pre", action="append", default=[], metavar="NAME=SHA")
    dg.add_argument(
        "--pull",
        action="append",
        default=[],
        metavar="NAME=STATE",
        help="One of updated|current|warn|cloned|none.",
    )
    dg.add_argument("--record", default="", metavar="PATH")
    dg.add_argument(
        "--repo",
        action="append",
        default=[],
        metavar="NAME=DIR",
        help="Repository checkout; repeatable, sets display order.",
    )
    dg.add_argument("--steps", type=int, default=3, metavar="N")
    dg.set_defaults(func=cmd_digest)

    kind = sub.add_parser(
        "kind",
        help="Predict the next build kind.",
    )
    kind.add_argument("-P", "--profile", default="local-install", metavar="NAME", help="Build profile.")
    kind.add_argument("-r", "--repo", required=True, metavar="DIR", help="Repository directory.")
    kind.add_argument("-s", "--step", required=True, metavar="NAME", help="Install step name.")
    kind.set_defaults(func=cmd_kind)

    mark = sub.add_parser(
        "mark-installed",
        help="Record a successful install watermark.",
    )
    mark.add_argument(
        "-d", "--dir",
        required=True,
        dest="repo_dir",
        metavar="DIR",
        help="Checkout directory.",
    )
    mark.add_argument(
        "-r", "--repo",
        required=True,
        metavar="NAME",
        help="Repository name (bob-cli, bob-plugins, bob-mac-capture).",
    )
    mark.set_defaults(func=cmd_mark_installed)

    record = sub.add_parser(
        "record",
        help="Append a timed install-step history row.",
    )
    record.add_argument("-i", "--incremental", choices=("on", "off", "unknown"), default="unknown", help="Cargo incremental mode.")
    record.add_argument("-k", "--kind", default="rebuild", metavar="NAME", help="Predicted build kind.")
    record.add_argument("-o", "--outcome", required=True, choices=("ok", "fail", "interrupted"), help="Step outcome.")
    record.add_argument("-p", "--phase", required=True, metavar="NAME", help="Recorded phase name.")
    record.add_argument("-P", "--profile", default="", metavar="NAME", help="Build profile.")
    record.add_argument("-t", "--secs", required=True, type=float, metavar="SECONDS", help="Elapsed seconds.")
    record.add_argument("-s", "--step", required=True, metavar="NAME", help="Install step name.")
    record.add_argument("-T", "--toolchain", default="unknown", metavar="TEXT", help="Toolchain identity.")
    record.add_argument("-w", "--waited", default=0.0, type=float, metavar="SECONDS", help="Time blocked on a build lock.")
    record.set_defaults(func=cmd_record)

    runner = sub.add_parser(
        "run",
        help="Run a build command with a live status line.",
    )
    runner.add_argument("-c", "--crate", metavar="NAME", help="Cargo crate to track (default: bob-obsidian).")
    runner.add_argument("-d", "--done", metavar="TEXT", help="Successful completion text.")
    runner.add_argument("-p", "--parser", required=True, choices=("cargo", "swiftpm"), help="Build output format.")
    runner.add_argument("-P", "--profile", metavar="NAME", help="Build profile.")
    runner.add_argument("-r", "--repo", required=True, metavar="DIR", help="Repository directory.")
    runner.add_argument("-R", "--rows-file", metavar="PATH", help="Write the number of scrollback rows.")
    runner.add_argument("-S", "--started-file", metavar="PATH", help="Touch after the child starts.")
    runner.add_argument("-s", "--step", required=True, metavar="NAME", help="Install step name.")
    runner.add_argument("command", nargs=argparse.REMAINDER, help="Child command after --.")
    runner.set_defaults(func=cmd_run)

    water = sub.add_parser(
        "watermark",
        help="Print a recorded install watermark.",
    )
    water.add_argument(
        "-j", "--json",
        action="store_true",
        help="Print the watermark as JSON.",
    )
    water.add_argument(
        "-r", "--repo",
        required=True,
        metavar="NAME",
        help="Repository name.",
    )
    water.set_defaults(func=cmd_watermark)
    return parser


def main(argv: Optional[Sequence[str]] = None) -> int:
    try:
        parser = _build_parser()
        args = parser.parse_args(list(argv) if argv is not None else None)
        return int(args.func(args) or 0)
    except KeyboardInterrupt:
        return 130
    except Exception:
        return _crash()


if __name__ == "__main__":
    sys.exit(main())
