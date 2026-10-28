#!/usr/bin/env python3
"""Compare optimized themes-list binaries on one frozen scratch vault.

Open an external vault with `sase repo open` first. This script copies only
Markdown and theme journals into the supplied workspace scratch directory;
it never runs Bob against or changes the source vault. Build both binaries
with the same release profile, including a clean pre-change revision.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import statistics
import subprocess
import time


def snapshot(source, dest):
    dest.mkdir(parents=True, exist_ok=False)
    digest = hashlib.sha256()
    count = 0
    size = 0
    for directory, dirs, files in os.walk(source, followlinks=False):
        dirs[:] = sorted(d for d in dirs if d != '.git' and not (Path(directory) / d).is_symlink())
        for name in sorted(files):
            path = Path(directory) / name
            if path.is_symlink():
                continue
            rel = path.relative_to(source)
            if path.suffix != '.md' and not (rel.parts[:2] == ('.bob', 'themes') and path.suffix == '.jsonl'):
                continue
            output = dest / rel
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, output)
            data = output.read_bytes()
            digest.update(str(rel).encode() + b'\0' + data + b'\0')
            count += 1
            size += len(data)
    return {'sha256': digest.hexdigest(), 'files': count, 'bytes': size}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--revised', type=Path, required=True)
    parser.add_argument('--source-vault', type=Path, required=True)
    parser.add_argument('--scratch', type=Path, required=True)
    parser.add_argument('--baseline-revision', required=True)
    parser.add_argument('--samples', type=int, default=100)
    parser.add_argument('--warmups', type=int, default=10)
    args = parser.parse_args()
    if args.samples < 100 or args.warmups < 10:
        parser.error('use at least 100 samples and 10 warmups per binary')
    binaries = {'baseline': args.baseline.resolve(), 'revised': args.revised.resolve()}
    binary_hashes = {key: hashlib.sha256(path.read_bytes()).hexdigest()
                     for key, path in binaries.items()}
    if binary_hashes['baseline'] == binary_hashes['revised']:
        parser.error('baseline and revised binaries are identical; rebuild each source before benchmarking')
    scratch = args.scratch.resolve()
    scratch.mkdir(parents=True, exist_ok=True)
    vault = scratch / 'vault'
    manifest = scratch / 'snapshot.json'
    if not manifest.exists():
        facts = snapshot(args.source_vault.resolve(), vault)
        manifest.write_text(json.dumps(facts, indent=2) + '\n')
    facts = json.loads(manifest.read_text())
    config = scratch / 'config.yml'
    config.write_text('plan:\n  exempt: [PLAN]\n  max_themes: 30\n')
    env = os.environ.copy()
    env.update(BOB_DIR=str(vault), BOB_NOW='2026-10-28 12:00:00',
               BOB_CONFIG_FILE=str(config), NO_COLOR='1', HOSTNAME='apollo')
    times = {key: [] for key in binaries}

    def run(key):
        command = [str(binaries[key]), 'themes', 'list', '-f', 'json']
        run_env = env | {'XDG_CACHE_HOME': str(scratch / (key + '-cache'))}
        started = time.perf_counter_ns()
        result = subprocess.run(command, env=run_env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=True)
        elapsed = (time.perf_counter_ns() - started) / 1e6
        if result.stderr:
            raise RuntimeError(result.stderr.decode())
        return elapsed

    for _ in range(args.warmups):
        for key in binaries:
            run(key)
    for iteration in range(args.samples):
        order = ['baseline', 'revised'] if iteration % 2 == 0 else ['revised', 'baseline']
        for key in order:
            times[key].append(run(key))
    summary = {key: {'median_ms': statistics.median(values),
                     'p95_ms': sorted(values)[math.ceil(len(values) * .95) - 1],
                     'samples_ms': values} for key, values in times.items()}
    delta = summary['revised']['p95_ms'] - summary['baseline']['p95_ms']
    result = {'host': platform.node(), 'baseline_revision': args.baseline_revision,
              'snapshot': facts, 'date': env['BOB_NOW'], 'config': config.read_text(),
              'samples_per_binary': args.samples, 'warmups_per_binary': args.warmups,
              'alternating': True, 'profile': 'release', 'results': summary,
              'delta_p95_ms': delta, 'bound_ms': 5, 'within_bound': delta <= 5,
              'binaries': binary_hashes}
    (scratch / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({key: value for key, value in result.items() if key != 'results'}))
    for key, values in summary.items():
        print(f"{key}: median={values['median_ms']:.3f}ms p95={values['p95_ms']:.3f}ms")
    print(f'delta p95: {delta:+.3f}ms (bound +5ms)')
    return 0 if delta <= 5 else 1


if __name__ == '__main__':
    raise SystemExit(main())
