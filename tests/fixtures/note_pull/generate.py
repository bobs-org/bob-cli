#!/usr/bin/env python3
"""Regenerate note-pull CLI outputs from Bob and the synthetic vault inputs."""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent
NOW = "2026-10-05 09:30:00"
MISSING_CONFIG = "/definitely/missing/bob-cli-note-pull-config.yml"
MOVE_DRAFT = (
    "@work\n\n@dev\n\n@work+gamma > @bob\n\n@dev+delta > @bob\n\n"
    "Docs push @work^docs-push@\n- ^beta"
)


def run_case(bob: Path, case: str, flags: list[str], draft: str, *, json_output: bool,
             dry_run: bool, expected_code: int, output_name: str) -> None:
    with tempfile.TemporaryDirectory(prefix=f"bob-note-pull-{case}-") as temporary:
        temp = Path(temporary)
        vault = temp / "vault"
        shutil.copytree(ROOT / "shared", vault)
        shutil.copytree(ROOT / case / "vault", vault, dirs_exist_ok=True)
        day = vault / "20261005.md"
        command = [str(bob), "capture", "-b", str(vault)]
        if json_output:
            command.extend(["-f", "json"])
        if dry_run:
            command.append("--dry-run")
        command.extend(flags)
        command.extend(["--", draft])
        environment = os.environ.copy()
        environment.update({
            "BOB_CONFIG_FILE": MISSING_CONFIG,
            "BOB_DAY_FILE": str(day),
            "BOB_DIR": str(vault),
            "BOB_NOW": NOW,
            "BOB_REF_JOBS_KICK": "off",
            "BOB_VAULT_SYNC_NOTIFY": "0",
            "NO_COLOR": "1",
        })
        result = subprocess.run(command, env=environment, capture_output=True, check=False)
        if result.returncode != expected_code:
            raise SystemExit(
                f"{case}: expected exit {expected_code}, got {result.returncode}\n"
                f"argv: {command!r}\nstdout:\n{result.stdout.decode(errors='replace')}\n"
                f"stderr:\n{result.stderr.decode(errors='replace')}"
            )
        output = result.stdout.decode("utf-8")
        output = output.replace(str(vault), "<VAULT>").replace(str(day), "<DAY_FILE>")
        destination = ROOT / case / output_name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(output, encoding="utf-8")


def main() -> None:
    repo = ROOT.parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--bob",
        type=Path,
        default=Path(os.environ.get("CARGO_TARGET_DIR", repo / "target"))
        / "debug"
        / "bob",
        help="bob executable built from this checkout",
    )
    args = parser.parse_args()
    bob = args.bob.resolve()
    if not bob.is_file():
        parser.error(f"bob executable does not exist: {bob}")

    run_case(bob, "np1", [], "@sase", json_output=True, dry_run=True,
             expected_code=0, output_name="stdout.json")
    run_case(bob, "np1", [], "@sase", json_output=False, dry_run=True,
             expected_code=0, output_name="human.txt")
    run_case(bob, "np2", ["-p", "all"], "@sase", json_output=True, dry_run=False,
             expected_code=0, output_name="stdout.json")
    run_case(bob, "np2", ["-p", "all"], "@sase", json_output=False, dry_run=False,
             expected_code=0, output_name="human.txt")
    run_case(bob, "np10-empty", [], "@sase", json_output=True, dry_run=False,
             expected_code=2, output_name="stdout.json")
    run_case(bob, "np10-folded", [], "@sase", json_output=True, dry_run=False,
             expected_code=2, output_name="stdout.json")
    run_case(bob, "np12", [], "@nosuch", json_output=True, dry_run=False,
             expected_code=2, output_name="stdout.json")
    run_case(bob, "np13", ["--require-pull-in-choice"], "@sase", json_output=True,
             dry_run=False, expected_code=1, output_name="stdout.json")
    run_case(bob, "np16", ["-p", "2", "--require-inbox-route-choice"], "@mac_inbox",
             json_output=True, dry_run=False, expected_code=1, output_name="choice.json")
    run_case(bob, "np16", ["-p", "2", "-i", "1=health"], "@mac_inbox",
             json_output=True, dry_run=False, expected_code=0, output_name="routed.json")
    # Later moves relocate pulled rows: groups and folds stay keyed by the
    # requested source (`offer.note`) while `note_path` reports the final home.
    run_case(bob, "np-move", [], MOVE_DRAFT, json_output=True, dry_run=True,
             expected_code=0, output_name="preview.json")
    run_case(bob, "np-move", ["-p", "all"], MOVE_DRAFT, json_output=True,
             dry_run=True, expected_code=0, output_name="picked.json")
    run_case(bob, "np-move", ["-p", "all"], MOVE_DRAFT, json_output=False,
             dry_run=True, expected_code=0, output_name="human.txt")
    run_case(bob, "np-move", ["--require-pull-in-choice"], MOVE_DRAFT,
             json_output=True, dry_run=False, expected_code=1,
             output_name="choice.json")
    run_case(bob, "np-move", [], "@work\n\n@dev", json_output=True,
             dry_run=True, expected_code=0, output_name="two-notes.json")


if __name__ == "__main__":
    main()
