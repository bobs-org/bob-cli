"""Run the strict Zensical or Material builder against assembled configuration."""

from __future__ import annotations

import shutil
import subprocess
import threading
from pathlib import Path
from typing import Iterator

from .assemble import assemble
from .context import DocsContext
from .errors import DocsError

STAGED_MANIFEST = "build.json"
_MIRROR_INTERVAL_SECONDS = 0.5


def build(
    root: Path, *, material: bool = False, production: bool = False, sources: Path | str | None = None
) -> int:
    context, manifest = assemble(
        root, production=production, sources=sources,
        builder="material" if material else "zensical",
    )
    return build_assembled(context, manifest, material=material)


def build_assembled(context: DocsContext, manifest: Path, *, material: bool = False) -> int:
    """Build from a source context already validated by :func:`assemble`."""

    executable = "mkdocs" if material else "zensical"
    command = [executable, "build", "--strict", "--config-file", str(context.assembled_config)]
    print("running: " + " ".join(command))
    try:
        result = subprocess.run(command, cwd=context.repo_root, check=False)
    except OSError as error:
        hint = " (install with `uv run --project tools/docs --group material ...`)" if material else ""
        raise DocsError(f"could not start {executable}{hint}: {error}") from error
    if result.returncode != 0:
        return result.returncode

    site_dir = context.site_dir
    if not site_dir.is_dir():
        raise DocsError(f"builder succeeded but site directory is missing: {site_dir}")
    shutil.copyfile(manifest, site_dir / "build.json")
    print(f"built site: {site_dir}")
    return 0


def _collect_authored_files(authored_docs: Path) -> dict[str, Path]:
    """Map staged-relative paths to real files, following in-docs symlinks."""

    resolved_root = authored_docs.resolve()
    collected: dict[str, Path] = {}

    def visit(directory: Path, prefix: str, ancestors: frozenset[Path]) -> None:
        for child in sorted(directory.iterdir()):
            rel = f"{prefix}{child.name}"
            if child.is_symlink():
                try:
                    target = child.resolve(strict=True)
                except OSError as error:
                    raise DocsError(f"docs source has a broken symlink: {child}: {error}") from error
                if not target.is_relative_to(resolved_root):
                    raise DocsError(f"docs source symlink escapes docs/: {child} -> {target}")
                if target.is_dir():
                    if target in ancestors:
                        raise DocsError(f"docs source has a symlink cycle: {child}")
                    visit(target, f"{rel}/", ancestors | {target})
                    continue
                collected[rel] = target
            elif child.is_dir():
                visit(child, f"{rel}/", ancestors)
            else:
                collected[rel] = child

    visit(authored_docs, "", frozenset({resolved_root}))
    return collected


def _iter_staged_files(staged_docs: Path) -> Iterator[tuple[str, Path]]:
    """Yield staged-relative paths for every file or symlink under the stage."""

    for path in sorted(staged_docs.rglob("*")):
        if path.is_symlink() or path.is_file():
            yield path.relative_to(staged_docs).as_posix(), path


def _needs_copy(real: Path, dest: Path) -> bool:
    if dest.is_symlink() or not dest.is_file():
        return True
    try:
        current, wanted = dest.stat(), real.stat()
    except OSError:
        return True
    if current.st_size != wanted.st_size:
        return True
    if current.st_mtime_ns == wanted.st_mtime_ns:
        return False
    try:
        return dest.read_bytes() != real.read_bytes()
    except OSError:
        return True


def mirror_docs_once(authored_docs: Path, staged_docs: Path) -> dict[str, int]:
    """Mirror one authored-to-staged pass; the staged manifest is preserved."""

    authored = _collect_authored_files(authored_docs)
    copied = 0
    for rel, real in authored.items():
        if rel == STAGED_MANIFEST:
            continue
        dest = staged_docs / Path(*rel.split("/"))
        if _needs_copy(real, dest):
            if dest.is_symlink() or dest.is_file():
                dest.unlink()
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(real, dest)
            copied += 1
    staged = dict(_iter_staged_files(staged_docs))
    deleted = 0
    for rel, path in staged.items():
        if rel == STAGED_MANIFEST or rel in authored:
            continue
        path.unlink()
        deleted += 1
    for path in sorted(staged_docs.rglob("*"), reverse=True):
        if path.is_dir() and not path.is_symlink() and path != staged_docs:
            try:
                path.rmdir()
            except OSError:
                pass
    return {"copied": copied, "deleted": deleted}


def _mirror_until_stopped(authored_docs: Path, staged_docs: Path, stop: threading.Event) -> None:
    while not stop.wait(_MIRROR_INTERVAL_SECONDS):
        try:
            mirror_docs_once(authored_docs, staged_docs)
        except (DocsError, OSError) as error:
            print(f"bob-docs: warning: live mirror skipped a pass: {error}")


def serve(root: Path) -> int:
    context, _manifest = assemble(root)
    stop = threading.Event()
    worker = threading.Thread(
        target=_mirror_until_stopped,
        args=(context.docs_root, context.output_dir / "docs", stop),
        daemon=True,
    )
    worker.start()
    try:
        command = ["zensical", "serve", "--config-file", str(context.assembled_config)]
        print("running: " + " ".join(command))
        try:
            return subprocess.run(command, cwd=context.repo_root, check=False).returncode
        except OSError as error:
            raise DocsError(f"could not start zensical serve: {error}") from error
    finally:
        stop.set()
        worker.join(timeout=5)
