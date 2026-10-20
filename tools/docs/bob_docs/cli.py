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
from .host import run_host_bootstrap, run_publish, run_status
from .links import (
    ONLINE_MAX_RETRIES,
    ONLINE_RETRY_WAIT_SECONDS,
    ONLINE_TIMEOUT_SECONDS,
    run_links,
)
from .lint import run_lint
from .publish_issue import (
    PUBLISH_BLOCKED_LABEL,
    PUBLISH_BLOCKED_TITLE,
    run_publish_report,
)
from .publish_plan import run_publish_plan
from .publish_record import parse_tool_assignments, run_publish_record
from .publish_verify import (
    DEFAULT_RETRIES,
    DEFAULT_TIMEOUT_SECONDS,
    run_publish_verify,
)
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
            "URL and link checks, search smoke, the executable tutorial, media "
            "determinism, and the bob-docs unit tests. "
            "--drill injects a staged-only broken link after the clean gates and "
            "requires the link gate to fail; a drill never succeeds or deploys."
        ),
    )
    check_parser.add_argument(
        "-d",
        "--drill",
        action="store_true",
        help="inject a staged-only broken link and require the link gate to fail",
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
        "gen",
        help="generate tracked contract indexes, memory-web reference pages, and mounted plugin inventory",
    )
    gen_parser.add_argument("-c", "--check", action="store_true", help="check generated files without writing")

    host_parser = subparsers.add_parser(
        "host-bootstrap",
        help="preview or apply the public documentation host repository",
        description=(
            "Create bobs-org/bobs-org.github.io if it is missing, sync the caller "
            "workflow and README, configure GitHub Pages (build_type=workflow), "
            "and enable Actions plus the publish workflow. Preview is the default "
            "and makes no mutations. Pass -y/--yes to apply. Never writes "
            "published.json. Standalone apply clones over SSH into a temporary "
            "directory; pass -d/--deploy-dir to use an explicit checkout instead."
        ),
    )
    host_parser.add_argument(
        "-d",
        "--deploy-dir",
        metavar="DIR",
        help="existing deploy-repo checkout to sync (skips the temporary SSH clone)",
    )
    host_parser.add_argument(
        "-y",
        "--yes",
        action="store_true",
        help="apply the printed create/sync/Pages/enable operations",
    )

    links_parser = subparsers.add_parser(
        "links",
        help="check built-site links with the pinned lychee release",
        description=(
            "Check the built site with the pinned lychee release. Offline is the "
            "default and blocks network requests; --online fetches external "
            "HTTP(S) destinations with bounded retries and timeouts while "
            "keeping fragment checking on the built tree."
        ),
    )
    links_parser.add_argument(
        "-o",
        "--online",
        action="store_true",
        help="fetch external HTTP(S) links with bounded retries and timeouts",
    )
    links_parser.add_argument(
        "-r",
        "--retries",
        metavar="N",
        type=int,
        default=ONLINE_MAX_RETRIES,
        help=f"maximum retries per online request (default: {ONLINE_MAX_RETRIES})",
    )
    links_parser.add_argument(
        "-t",
        "--timeout-seconds",
        metavar="SECONDS",
        type=int,
        default=ONLINE_TIMEOUT_SECONDS,
        help=f"timeout per online request (default: {ONLINE_TIMEOUT_SECONDS})",
    )
    links_parser.add_argument(
        "-w",
        "--wait-seconds",
        metavar="SECONDS",
        type=int,
        default=ONLINE_RETRY_WAIT_SECONDS,
        help=f"minimum wait between online retries (default: {ONLINE_RETRY_WAIT_SECONDS})",
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

    media_parser = subparsers.add_parser(
        "media",
        help="capture deterministic CLI SVGs from a synthetic vault",
        description=(
            "Run a small declared set of Bob commands under a POSIX PTY in the "
            "isolated tutorial environment, render ANSI through Rich, and write "
            "docs/assets/media/cli/. --check renders twice from independent "
            "sandboxes and compares the committed assets without writing tracked "
            "files. Requires Cargo or BOB_BIN."
        ),
    )
    media_parser.add_argument(
        "-c", "--check", action="store_true", help="check generated SVGs without writing"
    )

    publish_parser = subparsers.add_parser(
        "publish",
        help="enable, dispatch, and watch the host documentation publish workflow",
        description=(
            "Enable the deploy-repo publish workflow, dispatch it with optional "
            "force/drill/pinned flags, and watch the run that this dispatch "
            "started. A correlation id matches that run so a concurrent scheduled "
            "publish is not watched. A failed run keeps a nonzero exit status."
        ),
    )
    publish_parser.add_argument(
        "-d",
        "--drill",
        action="store_true",
        help="run the real gates with a staged broken link; never deploy or record",
    )
    publish_parser.add_argument(
        "-f",
        "--force",
        action="store_true",
        help="rebuild even when source inputs are unchanged",
    )
    publish_parser.add_argument(
        "-p",
        "--pinned",
        action="store_true",
        help="build the exact SHAs in the publication manifest, preserving hold",
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

    record_parser = subparsers.add_parser(
        "publish-record",
        help="write the durable published.json record from a planner candidate",
        description=(
            "Attach toolchain versions, build time, and the run URL to a "
            "next-publication.json candidate and write published.json. The hold "
            "flag is preserved; committing the manifest is a separate step."
        ),
    )
    record_parser.add_argument(
        "-b", "--built-at", metavar="ISO", help="build time (default: current UTC time)"
    )
    record_parser.add_argument(
        "-n", "--next", metavar="FILE", required=True, help="planner next-publication.json"
    )
    record_parser.add_argument(
        "-o", "--output", metavar="FILE", default="published.json", help="durable record to write"
    )
    record_parser.add_argument(
        "-r", "--run-url", metavar="URL", required=True, help="workflow run URL for provenance"
    )
    record_parser.add_argument(
        "-t",
        "--tool",
        metavar="KEY=VALUE",
        action="append",
        default=[],
        help="toolchain version (repeatable; default: collect installed versions)",
    )

    report_parser = subparsers.add_parser(
        "publish-report",
        help="open, update, or close the managed publish-blocked issue",
        description=(
            "Maintain exactly one managed blocked-publication issue with gh. "
            "--open creates the issue or comments on (and reopens) the existing "
            "one; --close comments and closes it after live verification."
        ),
    )
    report_parser.add_argument(
        "-c", "--close", action="store_true", help="close the open managed issue"
    )
    report_parser.add_argument(
        "-m", "--comment", metavar="TEXT", default="", help="close comment (with --close)"
    )
    report_parser.add_argument(
        "-d",
        "--detail",
        metavar="TEXT",
        action="append",
        default=[],
        help="extra body line for the blocked issue (repeatable, with --open)",
    )
    report_parser.add_argument(
        "-f", "--fingerprint", metavar="HEX", default="", help="publication fingerprint (with --open)"
    )
    report_parser.add_argument(
        "-l", "--label", metavar="LABEL", default=PUBLISH_BLOCKED_LABEL, help="managed issue label"
    )
    report_parser.add_argument(
        "-o", "--open", action="store_true", help="open or update the managed issue"
    )
    report_parser.add_argument(
        "-r", "--reason", metavar="TEXT", default="", help="failure reason (with --open)"
    )
    report_parser.add_argument(
        "-u", "--run-url", metavar="URL", default="", help="workflow run URL (with --open)"
    )
    report_parser.add_argument(
        "-s", "--step", metavar="STEP", default="", help="failed step name (with --open)"
    )
    report_parser.add_argument(
        "-t", "--title", metavar="TITLE", default=PUBLISH_BLOCKED_TITLE, help="managed issue title"
    )

    verify_parser = subparsers.add_parser(
        "publish-verify",
        help="verify the live site after a publication deploy",
        description=(
            "Check every built page URL live, resolve curated fragments against "
            "live pages with bounded retries, and confirm /build.json matches "
            "the deployed publication. Any failure exits nonzero."
        ),
    )
    verify_parser.add_argument(
        "-e", "--expected", metavar="FILE", required=True, help="planner next-publication.json"
    )
    verify_parser.add_argument(
        "-o", "--origin", metavar="URL", required=True, help="live site origin to verify"
    )
    verify_parser.add_argument(
        "-r",
        "--retries",
        metavar="N",
        type=int,
        default=DEFAULT_RETRIES,
        help=f"retries per live request (default: {DEFAULT_RETRIES})",
    )
    verify_parser.add_argument(
        "-s", "--site-urls", metavar="FILE", required=True, help="built site urls.txt"
    )
    verify_parser.add_argument(
        "-t",
        "--timeout-seconds",
        metavar="SECONDS",
        type=int,
        default=DEFAULT_TIMEOUT_SECONDS,
        help=f"timeout per live request (default: {DEFAULT_TIMEOUT_SECONDS})",
    )

    subparsers.add_parser("serve", help="assemble and serve the site with Zensical")

    social_parser = subparsers.add_parser("social-card", help="generate deterministic identity assets")
    social_parser.add_argument(
        "-c", "--check", action="store_true", help="check generated assets without writing"
    )

    subparsers.add_parser(
        "status",
        help="show host publication, live provenance, workflows, runs, hold, and issues",
        description=(
            "Report the deploy-repo published.json log, the live /build.json "
            "provenance, recent publish and weekly runs, workflow enabled state "
            "including disabled_inactivity, the hold flag, and open managed "
            "blocked issues. A missing first site is a clear status; GitHub "
            "authentication failure is an error. published.json is read from the "
            "deploy repository, not assumed at /published.json."
        ),
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
            return run_check(root, production=args.production, sources=args.sources, drill=args.drill)
        if args.command == "publish-record":
            record = run_publish_record(
                Path(args.next),
                Path(args.output),
                tools=parse_tool_assignments(args.tool) or None,
                built_at=args.built_at,
                run_url=args.run_url,
            )
            print(f"wrote {args.output} (fingerprint {record['fingerprint']})")
            return 0
        if args.command == "publish-report":
            if bool(args.open) == bool(args.close):
                print(
                    "bob-docs publish-report: pass exactly one of --open or --close",
                    file=sys.stderr,
                )
                return 2
            number = run_publish_report(
                mode="open" if args.open else "close",
                title=args.title,
                label=args.label,
                step=args.step,
                run_url=args.run_url,
                reason=args.reason,
                fingerprint=args.fingerprint,
                details=tuple(args.detail),
                comment=args.comment,
            )
            if number is None:
                print("managed issue is already clear; nothing to close")
            else:
                print(f"managed issue #{number}")
            return 0
        if args.command == "publish-verify":
            return run_publish_verify(
                root,
                origin=args.origin,
                site_urls_file=Path(args.site_urls),
                expected_file=Path(args.expected),
                timeout=args.timeout_seconds,
                retries=args.retries,
            )
        if args.command == "fonts":
            from .fonts import generate_fonts

            return generate_fonts(root, check=args.check)
        if args.command == "gen":
            return check_or_generate(root, check=args.check)
        if args.command == "host-bootstrap":
            return run_host_bootstrap(root, yes=args.yes, deploy_dir=args.deploy_dir)
        if args.command == "links":
            return run_links(
                root,
                online=args.online,
                timeout=args.timeout_seconds,
                retries=args.retries,
                retry_wait=args.wait_seconds,
            )
        if args.command == "lint":
            context, _manifest = assemble(root)
            return run_lint(root, fix=args.fix, docs_context=context)
        if args.command == "media":
            from .media import run_media

            return run_media(root, check=args.check)
        if args.command == "publish":
            return run_publish(force=args.force, drill=args.drill, pinned=args.pinned)
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
        if args.command == "status":
            return run_status()
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
