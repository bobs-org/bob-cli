"""Command-line interface for the Bob documentation toolchain."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path
from typing import Sequence

from .assemble import assemble
from .build import build, serve
from .errors import DocsError
from .gen_contracts import check_or_generate


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="bob-docs",
        description="Assemble, generate, and build Bob's documentation site.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("assemble", help="assemble config and source provenance")
    build_parser = subparsers.add_parser("build", help="build the strict documentation site")
    build_parser.add_argument("-m", "--material", action="store_true", help="build with Material 9.7")

    gen_parser = subparsers.add_parser("gen", help="generate tracked contract indexes")
    gen_parser.add_argument("-c", "--check", action="store_true", help="check generated files without writing")

    subparsers.add_parser("serve", help="assemble and serve the site with Zensical")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    root = Path.cwd()
    try:
        if args.command == "assemble":
            context, _manifest = assemble(root)
            print(f"assembled config: {context.assembled_config}")
            return 0
        if args.command == "build":
            return build(root, material=args.material)
        if args.command == "gen":
            return check_or_generate(root, check=args.check)
        if args.command == "serve":
            return serve(root)
    except DocsError as error:
        print(f"bob-docs: error: {error}", file=sys.stderr)
        return 2
    return 2
