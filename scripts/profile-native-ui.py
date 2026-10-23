#!/usr/bin/env python3
"""Compare frozen offline X11 previews; optional glibc allocation diagnostics, never an account."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import statistics
import struct
import subprocess
import sys
import tempfile
import time

import psutil


ALLOCATOR = r'''
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <unistd.h>
extern void *__libc_malloc(size_t);
extern void *__libc_calloc(size_t, size_t);
extern void *__libc_realloc(void *, size_t);
extern void __libc_free(void *);
struct counts { _Atomic uint64_t mallocs, callocs, reallocs, frees, requested; };
static _Atomic(struct counts *) active;
__attribute__((constructor)) static void init(void) {
    const char *path = getenv("SEREIN_ALLOC_PROFILE");
    if (!path) return;
    int fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    if (fd < 0) return;
    if (ftruncate(fd, sizeof(struct counts)) == 0) {
        void *p = mmap(0, sizeof(struct counts), PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        if (p != MAP_FAILED) atomic_store_explicit(&active, p, memory_order_relaxed);
    }
    close(fd);
}
void *malloc(size_t n) {
    struct counts *p = atomic_load_explicit(&active, memory_order_relaxed);
    if (p) { atomic_fetch_add_explicit(&p->mallocs, 1, memory_order_relaxed); atomic_fetch_add_explicit(&p->requested, n, memory_order_relaxed); }
    return __libc_malloc(n);
}
void *calloc(size_t n, size_t s) {
    struct counts *p = atomic_load_explicit(&active, memory_order_relaxed);
    if (p) { atomic_fetch_add_explicit(&p->callocs, 1, memory_order_relaxed); if (!s || n <= SIZE_MAX / s) atomic_fetch_add_explicit(&p->requested, n * s, memory_order_relaxed); }
    return __libc_calloc(n, s);
}
void *realloc(void *v, size_t n) {
    struct counts *p = atomic_load_explicit(&active, memory_order_relaxed);
    if (p) { atomic_fetch_add_explicit(&p->reallocs, 1, memory_order_relaxed); atomic_fetch_add_explicit(&p->requested, n, memory_order_relaxed); }
    return __libc_realloc(v, n);
}
void free(void *v) {
    struct counts *p = atomic_load_explicit(&active, memory_order_relaxed);
    if (p && v) atomic_fetch_add_explicit(&p->frees, 1, memory_order_relaxed);
    __libc_free(v);
}
'''


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def counters(path):
    return struct.unpack('=5Q', path.read_bytes())


def sample(binary, workload, output, preload=None, reaction_history=False):
    page = ('conversation' if workload.startswith('conversation-') else
            'server-members' if workload == 'server-members-search' else workload.removesuffix('-active'))
    active = workload in ('conversation-churn', 'gif-favorites', 'server-members-search') or workload.endswith('-active')
    metrics = output.with_suffix('.metrics.json')
    allocation = output.with_suffix('.counts')
    env = os.environ.copy()
    env['WINIT_X11_SCALE_FACTOR'] = '1'
    if preload:
        env['LD_PRELOAD'] = str(preload)
        env['SEREIN_ALLOC_PROFILE'] = str(allocation)
    arguments = [str(binary), '--demo', '--interactive', '--page=' + page,
                 '--width=1120', '--height=760']
    if reaction_history:
        arguments.append('--reaction-history')
    if active:
        arguments.append('--metrics=' + str(metrics))
    if workload == 'conversation-churn':
        arguments.append('--timeline-churn')
    log_path = output.with_suffix('.log')
    with log_path.open('w') as log:
        child = subprocess.Popen(arguments, env=env, stdout=log, stderr=log)
        def alive():
            if child.poll() is not None:
                raise RuntimeError('Preview exited; see ' + str(log_path))
        try:
            process = psutil.Process(child.pid)
            for _ in range(30):
                alive()
                time.sleep(.1)
            windows = subprocess.check_output(['xdotool', 'search', '--onlyvisible', '--pid', str(child.pid)],
                                              text=True, timeout=5).splitlines()
            if not windows:
                raise RuntimeError('No visible preview; see ' + str(log_path))
            window = windows[-1]
            subprocess.run(['xdotool', 'windowfocus', '--sync', window], check=True, timeout=5)
            if reaction_history:
                subprocess.run(['xdotool', 'mousemove', '--window', window, '600', '400'],
                               check=True, timeout=5)
                time.sleep(.5)
            before = counters(allocation) if preload else None
            cpu = process.cpu_times()
            cpu_start = cpu.user + cpu.system
            started = time.monotonic()
            rss, helper_peak = [], 0
            searches = 0
            while time.monotonic() - started < 5:
                alive()
                rss.append(process.memory_info().rss)
                helper_peak = max(helper_peak, sum(p.memory_info().rss for p in process.children(recursive=True) if p.is_running()))
                if workload == 'gif-favorites':
                    button = '4' if (len(rss) // 10) % 2 == 0 else '5'
                    subprocess.run(['xdotool', 'mousemove', '--window', window, '600', '400', 'click', button],
                                   check=True, stdout=subprocess.DEVNULL, timeout=5)
                if workload == 'server-members-search' and time.monotonic() - started >= searches:
                    # Verified common member-search bounds in both frozen 1120x760 fixtures.
                    subprocess.run(['xdotool', 'mousemove', '--window', window, '420', '313',
                                    'click', '1', 'key', '--clearmodifiers', 'ctrl+a', 'BackSpace'],
                                   check=True, stdout=subprocess.DEVNULL, timeout=5)
                    if searches % 2 == 0:
                        subprocess.run(['xdotool', 'type', '--clearmodifiers', '--delay', '0', 'Avery'],
                                       check=True, stdout=subprocess.DEVNULL, timeout=5)
                    searches += 1
                time.sleep(.1)
            alive()
            after = counters(allocation) if preload else None
            cpu = process.cpu_times()
            elapsed = time.monotonic() - started
            settled = []
            for _ in range(20):
                alive()
                settled.append(process.memory_info().rss)
                time.sleep(.1)
            report = json.loads(metrics.read_text()) if active else dict(samples=[])
            callbacks = sorted(row[1] / 1_000_000 for row in report['samples'])
            if active and len(callbacks) < 5:
                raise RuntimeError('Insufficient callbacks; see ' + str(metrics))
            def percentile(q):
                return callbacks[min(len(callbacks) - 1, int((len(callbacks) - 1) * q))] if callbacks else None
            result = dict(renderer=next((line for line in log_path.read_text().splitlines()
                                         if line.startswith('Offline preview renderer:')), None),
                          ui_callback_samples=len(callbacks), ui_callback_p50_ms=percentile(.5),
                          ui_callback_p95_ms=percentile(.95), ui_wheel_events=sum(row[2] for row in report['samples']),
                          cpu_one_core_percent=100 * (cpu.user + cpu.system - cpu_start) / elapsed,
                          peak_rss_bytes=max(rss + settled), settled_rss_bytes=statistics.median(settled),
                          helper_peak_rss_bytes=helper_peak, samples=len(rss), elapsed_seconds=elapsed)
            if workload == 'server-members-search':
                result['member_search_edits'] = searches
            if preload:
                result['allocation_calls'] = dict(zip(['malloc', 'calloc', 'realloc', 'free', 'requested_bytes'],
                                                     (end - begin for begin, end in zip(before, after))))
                if result['allocation_calls']['malloc'] == 0:
                    raise RuntimeError('Allocator interposition did not observe calls')
            output.write_text(json.dumps(result, indent=2) + '\n')
            return result
        finally:
            child.terminate()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--only', choices=['language', 'gif-favorites', 'conversation-idle', 'conversation-churn',
                                          'server-members', 'server-roles', 'server-moderation',
                                          'server-members-active', 'server-roles-active',
                                          'server-members-search',
                                          'channels', 'mod-view', 'server-applications', 'server-access'], required=True)
    parser.add_argument('--reaction-history', action='store_true', help='Hover the conversation with synthetic account ranking; older previews lack this fixture')
    parser.add_argument('--allocation-profile', action='store_true', help='One instrumented GIF diagnostic per build; never use its CPU timings for acceptance')
    parser.add_argument('binaries', nargs='+', help='LABEL=FROZEN_EXECUTABLE')
    args = parser.parse_args()
    if sys.platform != 'linux' or not os.environ.get('DISPLAY'):
        parser.error('This sampler requires a controlled Linux X11 display')
    if args.allocation_profile and (args.only != 'gif-favorites' or platform.libc_ver()[0] != 'glibc'):
        parser.error('Allocation diagnostics require glibc and the GIF workload')
    binaries = {}
    for argument in args.binaries:
        label, name = argument.split('=', 1)
        if not re.fullmatch(r'[a-z][a-z0-9_-]{0,31}', label) or label in binaries:
            parser.error('Use unique simple labels')
        path = Path(name).resolve(strict=True)
        if not path.is_file():
            parser.error('Each binary must be a file')
        binaries[label] = path
    os.sched_setaffinity(0, {min(os.sched_getaffinity(0))})
    args.output.parent.mkdir(parents=True, exist_ok=True)
    traces = args.output.parent / (args.output.stem + '-traces')
    traces.mkdir(exist_ok=True)
    metadata = {label: dict(path=str(path), sha256=digest(path)) for label, path in binaries.items()}
    result = dict(environment=dict(os=platform.platform(), viewport=[1120, 760], scale=1,
                                   warmup_seconds=3, sample_seconds=5, settle_seconds=2, sample_interval_seconds=.1,
                                   affinity='one inherited logical CPU', display=os.environ['DISPLAY'],
                                   reaction_history=args.reaction_history,
                                   hover_settle_seconds=.5 if args.reaction_history else 0,
                                   method='Fresh offline native processes; UI callbacks are MessagingUi::show, not presented frames or input latency'),
                  binaries=metadata, workloads={})
    if args.only == 'server-members-search':
        result['environment']['member_search'] = dict(position=[420, 313],
            interval_seconds=1, alternating=['Avery', ''], fixture_members=8,
            method='Native text input and debounced fixture replies; callbacks do not measure end-to-end search latency')
    with tempfile.TemporaryDirectory(prefix='serein-allocation-profile-') as temporary:
        preload = None
        if args.allocation_profile:
            source = Path(temporary) / 'allocator.c'
            source.write_text(ALLOCATOR)
            preload = Path(temporary) / 'allocator.so'
            subprocess.run(['cc', '-O2', '-std=c11', '-D_POSIX_C_SOURCE=200809L', '-shared', '-fPIC',
                            str(source), '-o', str(preload)], check=True, timeout=30)
            result['environment'].update(warmup_runs=0, measured_runs=1,
                                         diagnostic='Interposed malloc/calloc/realloc/free calls across the process, including native libraries; excludes aligned/direct allocation paths. Requested bytes are traffic, not live heap. Instrumented CPU/timings are not acceptance results.')
            result['allocator_source_sha256'] = hashlib.sha256(ALLOCATOR.encode()).hexdigest()
        else:
            result['environment'].update(warmup_runs=1, measured_runs=5)
            if args.output.exists():
                previous = json.loads(args.output.read_text())
                if previous['binaries'] != metadata or previous['environment'].get('reaction_history', False) != args.reaction_history:
                    raise RuntimeError('Resuming requires the same frozen binaries and fixture flags')
                result = previous
            if args.only in result['workloads']:
                return
        records = {label: [] for label in binaries}
        labels = list(binaries)
        for iteration in range(1 if preload else 6):
            offset = iteration % len(labels)
            for label in labels[offset:] + labels[:offset]:
                row = sample(binaries[label], args.only, traces / f'{args.only}-{label}-{iteration}.json', preload, args.reaction_history)
                if preload or iteration:
                    records[label].append(row)
        result['workloads'][args.only] = {
            label: dict(samples=rows, median={key: statistics.median(row[key] for row in rows)
                                            for key, value in rows[0].items() if isinstance(value, (int, float))})
            for label, rows in records.items()}
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print('Completed ' + args.only + (' allocation diagnostic' if args.allocation_profile else ' matched comparison'), flush=True)


if __name__ == '__main__':
    main()
