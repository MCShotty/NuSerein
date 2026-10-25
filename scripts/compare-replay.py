#!/usr/bin/env python3
"""Compare bounded offline reducer workloads; never launch the desktop or contact Discord."""
import argparse
import hashlib
import json
import os
import platform
import re
import statistics
import subprocess
import tempfile
import urllib.request
from pathlib import Path

PARENT = 'https://github.com/ViceVerse-cz/Serein.git'
RELEASES = 'https://api.github.com/repos/ViceVerse-cz/Serein/releases?per_page=20'
TAG = re.compile(r'v\d+\.\d+\.\d+-nightly\.\d{8}\.\d+')
SHA = re.compile(r'[0-9a-f]{40}')
SETTINGS = dict(CARGO_PROFILE_RELEASE_OPT_LEVEL='3', CARGO_PROFILE_RELEASE_LTO='thin',
                CARGO_PROFILE_RELEASE_CODEGEN_UNITS='1', CARGO_PROFILE_RELEASE_DEBUG='line-tables-only',
                CARGO_PROFILE_RELEASE_STRIP='symbols', CARGO_PROFILE_RELEASE_INCREMENTAL='false',
                CARGO_PROFILE_RELEASE_PANIC='abort', CARGO_BUILD_JOBS='2')


def command(args, cwd, env=None, capture=False):
    return subprocess.run(args, cwd=cwd, env=env, check=True, text=True, encoding='utf-8',
                          stdout=subprocess.PIPE if capture else None,
                          timeout=900).stdout


def latest_nightly():
    headers = {'Accept': 'application/vnd.github+json', 'User-Agent': 'Serein-offline-replay'}
    token = os.environ.get('GH_TOKEN') or os.environ.get('GITHUB_TOKEN')
    if token:
        headers['Authorization'] = 'Bearer ' + token
    with urllib.request.urlopen(urllib.request.Request(RELEASES, headers=headers), timeout=30) as response:
        data = response.read(4 * 1024 * 1024 + 1)
    if len(data) > 4 * 1024 * 1024:
        raise RuntimeError('Release metadata exceeds comparison limit')
    releases = json.loads(data)
    candidates = [release for release in releases if not release.get('draft')
                  and TAG.fullmatch(release.get('tag_name', ''))]
    if not candidates:
        raise RuntimeError('No current published nightly found; no comparison is claimed')
    return max(candidates, key=lambda release: release['published_at'])['tag_name']


def pin_sampling():
    if hasattr(os, 'sched_getaffinity'):
        cpu = min(os.sched_getaffinity(0))
        os.sched_setaffinity(0, {cpu})
        return f'logical CPU {cpu}'
    if os.name == 'nt':
        import ctypes
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.GetCurrentProcess.restype = ctypes.c_void_p
        kernel.GetProcessAffinityMask.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_size_t), ctypes.POINTER(ctypes.c_size_t)]
        kernel.SetProcessAffinityMask.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
        process, system = ctypes.c_size_t(), ctypes.c_size_t()
        handle = kernel.GetCurrentProcess()
        if kernel.GetProcessAffinityMask(handle, ctypes.byref(process), ctypes.byref(system)):
            mask = process.value & -process.value
            if mask and kernel.SetProcessAffinityMask(handle, mask):
                return f'logical CPU mask {mask}'
        return 'unavailable; Windows scheduler'
    return 'unavailable; platform scheduler'


def sample(binary, args, cwd, env):
    result = subprocess.run([str(binary), *args], cwd=cwd, env=env, check=True,
                            capture_output=True, text=True, encoding='utf-8', timeout=120)
    match = re.search(r' in ([0-9.]+)(ns|µs|ms|s);', result.stdout)
    if not match or len(result.stdout) > 16384 or len(result.stderr) > 16384:
        raise RuntimeError('Unexpected reducer output; no timing is inferred')
    return float(match[1]) * {'ns': .000001, 'µs': .001, 'ms': 1, 's': 1000}[match[2]]


def binary_provenance(binary):
    with binary.open('rb') as stream:
        return dict(bytes=binary.stat().st_size,
                    sha256=hashlib.file_digest(stream, 'sha256').hexdigest())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fork-base', required=True)
    parser.add_argument('--checkpoint', default='', help='Optional immutable pre-change PR head')
    parser.add_argument('--nightly-tag', default='', help='Optional published nightly pinned for this comparison')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root = Path(command(['git', 'rev-parse', '--show-toplevel'], Path.cwd(), capture=True).strip())
    base = args.fork_base
    if not SHA.fullmatch(base) or int(base, 16) == 0:
        raise ValueError('A nonzero full previous-fork revision is required')
    if args.checkpoint and (not SHA.fullmatch(args.checkpoint) or int(args.checkpoint, 16) == 0):
        raise ValueError('A nonzero full checkpoint revision is required')
    if args.nightly_tag and not TAG.fullmatch(args.nightly_tag):
        raise ValueError('A valid published-nightly tag is required')
    if command(['git', 'status', '--porcelain'], root, capture=True).strip():
        raise RuntimeError('Commit the reviewed changes before recording a revision-labelled comparison')
    current = command(['git', 'rev-parse', 'HEAD'], root, capture=True).strip()
    tag = args.nightly_tag or latest_nightly()
    env = dict(os.environ, **SETTINGS)
    # Only the release lookup needs a repository token; build scripts and benchmarks do not.
    for key in ('GH_TOKEN', 'GITHUB_TOKEN'):
        env.pop(key, None)
    command(['git', 'fetch', '--no-tags', '--depth=1', PARENT, 'refs/tags/' + tag], root, env)
    nightly = command(['git', 'rev-parse', 'FETCH_HEAD^{commit}'], root, env, capture=True).strip()
    for revision in {base, args.checkpoint} - {''}:
        available = subprocess.run(['git', 'cat-file', '-e', revision + '^{commit}'], cwd=root, env=env, capture_output=True, timeout=10).returncode == 0
        if not available:
            command(['git', 'fetch', '--no-tags', '--depth=1', 'origin', revision], root, env)
    revisions = {'nightly': nightly, 'previous_fork': base}
    if args.checkpoint:
        revisions['starting_pr'] = args.checkpoint
    revisions['after'] = current
    compiler = command(['rustc', '+1.98.1', '--version'], root, env, capture=True).strip()
    result = dict(revisions=revisions, nightly_tag=tag, os=platform.platform(),
                  cpu=platform.processor(), logical_cpus=os.cpu_count(), rust=compiler,
                  release_settings=SETTINGS, samples=5, warmup_runs=1, workloads={},
                  limits='Synthetic core reducer only; no UI/media throughput, RSS or live interoperability claim.')
    worktrees = []
    with tempfile.TemporaryDirectory(prefix='serein-parity-replay-', dir=os.environ.get('RUNNER_TEMP')) as temporary:
        folder = Path(temporary)
        try:
            binaries = {}
            for label, revision in revisions.items():
                source = folder / (label + '-source')
                command(['git', 'worktree', 'add', '--detach', str(source), revision], root, env)
                worktrees.append(source)
                target = folder / (label + '-target')
                build_env = dict(env, CARGO_TARGET_DIR=str(target))
                command(['cargo', '+1.98.1', 'build', '--release', '--locked', '-p', 'replay-bench'], source, build_env)
                binaries[label] = target / 'release' / ('replay-bench.exe' if os.name == 'nt' else 'replay-bench')
            result['binary_provenance'] = {label: binary_provenance(binary)
                                           for label, binary in binaries.items()}
            result['sampling_affinity'] = pin_sampling()
            for name, switches in [('reducer', []), ('wide_channels', ['--wide-channels'])]:
                values = {label: [] for label in binaries}
                for iteration in range(6):
                    labels = list(binaries)
                    offset = iteration % len(labels)
                    for label in labels[offset:] + labels[:offset]:
                        milliseconds = sample(binaries[label], switches, root, env)
                        if iteration:
                            values[label].append(milliseconds)
                result['workloads'][name] = {label: dict(median_ms=statistics.median(rows),
                    min_ms=min(rows), max_ms=max(rows), samples_ms=rows) for label, rows in values.items()}
        finally:
            for source in reversed(worktrees):
                command(['git', 'worktree', 'remove', str(source)], root, env)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
