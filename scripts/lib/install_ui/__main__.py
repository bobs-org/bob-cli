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

from install_ui import state, term  # noqa: E402


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


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="install_ui",
        description="Terminal UI helper for just install and just install-all.",
    )
    sub = parser.add_subparsers(dest="command", metavar="command")
    sub.required = True

    mark = sub.add_parser(
        "mark-installed",
        help="Record a successful install watermark.",
    )
    mark.add_argument(
        "--repo",
        required=True,
        metavar="NAME",
        help="Repository name (bob-cli, bob-plugins, bob-mac-capture).",
    )
    mark.add_argument(
        "--dir",
        required=True,
        dest="repo_dir",
        metavar="DIR",
        help="Checkout directory.",
    )
    mark.set_defaults(func=cmd_mark_installed)

    water = sub.add_parser(
        "watermark",
        help="Print a recorded install watermark.",
    )
    water.add_argument(
        "--repo",
        required=True,
        metavar="NAME",
        help="Repository name.",
    )
    water.add_argument(
        "--json",
        action="store_true",
        help="Print the watermark as JSON.",
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
