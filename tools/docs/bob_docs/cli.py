"""Command-line interface for the Bob documentation toolchain."""

from __future__ import annotations

import argparse
import json
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
from .publish_plan import run_publish_plan
from .urls import run_urls


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="bob-docs",
        description="Assemble, generate, lint, and build Bob's documentation site.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    a11y_parser = subparsers.add_parser("a11y", help="audit the built site with Playwright and axe")
    a11y_parser.add_argument(
        "-b", "--builder", metavar="BUILDER", help="builder report to audit (default: existing site)"
    )
    a11y_parser.add_argument(
        "-s", "--site", metavar="DIR", help="built site directory to audit (default: target/docs/site)"
    )

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

    fonts_parser = subparsers.add_parser("fonts", help="subset bundled fonts to WOFF2")
    fonts_parser.add_argument(
        "-c", "--check", action="store_true", help="check generated fonts without writing"
    )

    gen_parser = subparsers.add_parser(
        "gen", help="generate tracked contract indexes and memory-web reference pages"
    )
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

    publish_plan_parser = subparsers.add_parser(
        "publish-plan",
        help="select immutable publication sources and write a build plan",
        description=(
            "Resolve the selected source commits, compare their documentation inputs "
            "with the publication manifest, and write transient planner outputs. "
            "This command never changes the publication manifest."
        ),
    )
    publish_plan_parser.add_argument(
        "-d", "--drill", action="store_true", help="run gates without deploying or recording publication"
    )
    publish_plan_parser.add_argument(
        "-f", "--force", action="store_true", help="rebuild even when source inputs are unchanged"
    )
    publish_plan_parser.add_argument(
        "-g", "--github-output", metavar="FILE", help="append stable workflow outputs to FILE"
    )
    publish_plan_parser.add_argument(
        "-m", "--manifest", metavar="FILE", default="published.json", help="publication state manifest"
    )
    publish_plan_parser.add_argument(
        "-o", "--output-dir", metavar="DIR", default="target/docs/publish-plan", help="transient plan output directory"
    )
    publish_plan_parser.add_argument(
        "-p", "--pinned", action="store_true", help="build the exact sources in the valid publication manifest"
    )
    publish_plan_parser.add_argument(
        "-r", "--recovery", action="store_true", help="re-run gates for an open blocked publication"
    )
    publish_plan_parser.add_argument(
        "-w", "--work-dir", metavar="DIR", default="target/docs/publish-work", help="dedicated clean source checkout directory"
    )

    subparsers.add_parser("serve", help="assemble and serve the site with Zensical")

    social_parser = subparsers.add_parser("social-card", help="generate deterministic identity assets")
    social_parser.add_argument(
        "-c", "--check", action="store_true", help="check generated assets without writing"
    )

    subparsers.add_parser(
        "tutorial",
        help="execute the published Get started Bash fences in a synthetic vault",
        description=(
            "Run every executable Bash fence in the Get started pages, in navigation order, "
            "against a fresh synthetic vault. Requires Cargo or BOB_BIN."
        ),
    )

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
        if args.command == "a11y":
            from .a11y import run_a11y

            return run_a11y(root, builder=args.builder, site=args.site)
        if args.command == "assemble":
            context, _manifest = assemble(root, production=args.production, sources=args.sources)
            print(f"assembled config: {context.assembled_config}")
            return 0
        if args.command == "build":
            return build(root, material=args.material, production=args.production, sources=args.sources)
        if args.command == "check":
            return run_check(root, production=args.production, sources=args.sources)
        if args.command == "fonts":
            from .fonts import generate_fonts

            return generate_fonts(root, check=args.check)
        if args.command == "gen":
            return check_or_generate(root, check=args.check)
        if args.command == "links":
            return run_links(root)
        if args.command == "lint":
            context, _manifest = assemble(root)
            return run_lint(root, fix=args.fix, docs_context=context)
        if args.command == "publish-plan":
            plan = run_publish_plan(
                root,
                manifest=Path(args.manifest),
                output_dir=Path(args.output_dir),
                work_dir=Path(args.work_dir),
                force=args.force,
                drill=args.drill,
                pinned=args.pinned,
                recovery=args.recovery,
                github_output=Path(args.github_output) if args.github_output else None,
            )
            print(json.dumps(plan, sort_keys=True, separators=(",", ":")))
            return 0
        if args.command == "serve":
            return serve(root)
        if args.command == "social-card":
            from .assets import generate_assets

            return generate_assets(root, check=args.check)
        if args.command == "tutorial":
            from .tutorial import run_tutorial

            return run_tutorial(root)
        if args.command == "urls":
            return run_urls(
                root, refresh=args.refresh, production=args.production, sources=args.sources
            )
    except DocsError as error:
        print(f"bob-docs: error: {error}", file=sys.stderr)
        return 2
    return 2
