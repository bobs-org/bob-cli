"""Command-line interface for the Bob documentation toolchain."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path
from typing import Sequence

from .assemble import assemble
from .build import build, serve
from .check import run_check
from .errors import DocsError
from .gen_contracts import check_or_generate
from .links import run_links
from .lint import run_lint
from .urls import run_urls


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="bob-docs",
        description="Assemble, generate, lint, and build Bob's documentation site.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    assemble_parser = subparsers.add_parser(
        "assemble", help="assemble config and source provenance"
    )
    assemble_parser.add_argument(
        "-p", "--production", action="store_true", help="verify pinned production sources"
    )
    assemble_parser.add_argument(
        "-s", "--sources", metavar="FILE", help="production sources JSON (needs --production)"
    )
    build_parser = subparsers.add_parser("build", help="build the strict documentation site")
    build_parser.add_argument("-m", "--material", action="store_true", help="build with Material 9.7")
    build_parser.add_argument(
        "-p", "--production", action="store_true", help="verify pinned production sources"
    )
    build_parser.add_argument(
        "-s", "--sources", metavar="FILE", help="production sources JSON (needs --production)"
    )
    check_parser = subparsers.add_parser(
        "check",
        help="run the complete local documentation gate",
        description=(
            "Assemble once, then run lint, generated drift, strict Zensical build, "
            "URL and link checks, search smoke, and the bob-docs unit tests."
        ),
    )
    check_parser.add_argument(
        "-p", "--production", action="store_true", help="verify pinned production sources"
    )
    check_parser.add_argument(
        "-s", "--sources", metavar="FILE", help="production sources JSON (needs --production)"
    )

    gen_parser = subparsers.add_parser("gen", help="generate tracked contract indexes")
    gen_parser.add_argument("-c", "--check", action="store_true", help="check generated files without writing")

    subparsers.add_parser(
        "links",
        help="check built-site links offline with the pinned lychee release",
    )

    lint_parser = subparsers.add_parser(
        "lint",
        help="assemble and enforce the documentation contract",
        description=(
            "Assemble the current source context, then check typing, the contract registry, front matter, budgets, hygiene, "
            "test pins, portability, navigation, privacy, and media provenance. "
            "Normal runs never write tracked files. --fix only ratchets page "
            "ceilings down and removes obsolete privacy baseline hits."
        ),
    )
    lint_parser.add_argument(
        "-f",
        "--fix",
        action="store_true",
        help="lower page ceilings and drop obsolete privacy baseline hits; never raise limits",
    )

    subparsers.add_parser("serve", help="assemble and serve the site with Zensical")

    urls_parser = subparsers.add_parser("urls", help="verify published URLs and page mentions")
    urls_parser.add_argument(
        "-p", "--production", action="store_true", help="verify pinned production sources"
    )
    urls_parser.add_argument(
        "-r", "--refresh", action="store_true", help="add observed site URLs to the curated contract"
    )
    urls_parser.add_argument(
        "-s", "--sources", metavar="FILE", help="production sources JSON (needs --production)"
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    root = Path.cwd()
    try:
        if args.command == "assemble":
            context, _manifest = assemble(root, production=args.production, sources=args.sources)
            print(f"assembled config: {context.assembled_config}")
            return 0
        if args.command == "build":
            return build(root, material=args.material, production=args.production, sources=args.sources)
        if args.command == "check":
            return run_check(root, production=args.production, sources=args.sources)
        if args.command == "gen":
            return check_or_generate(root, check=args.check)
        if args.command == "links":
            return run_links(root)
        if args.command == "lint":
            context, _manifest = assemble(root)
            return run_lint(root, fix=args.fix, docs_context=context)
        if args.command == "serve":
            return serve(root)
        if args.command == "urls":
            return run_urls(
                root, refresh=args.refresh, production=args.production, sources=args.sources
            )
    except DocsError as error:
        print(f"bob-docs: error: {error}", file=sys.stderr)
        return 2
    return 2
