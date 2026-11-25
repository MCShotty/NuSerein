"""Capture the native offline preview's own page list; never start the signed-in desktop.

Run inside one Xvfb display on Linux, with the already-built profile_preview example.
The output directory must not exist. A failed capture does not masquerade as coverage.
"""
import argparse
import json
from pathlib import Path
import re
import struct
import subprocess


def capture(binary, output):
    binary = binary.resolve(strict=True)
    if binary.stem != 'profile_preview':
        raise ValueError('expected the offline profile_preview example')
    pages = subprocess.check_output([binary, '--demo', '--list-pages'], text=True, timeout=10).splitlines()
    if not 1 <= len(pages) <= 100 or len(set(pages)) != len(pages) or not all(re.fullmatch('[a-z][a-z0-9-]{0,47}', p) for p in pages):
        raise ValueError('invalid preview page list')
    output.mkdir(parents=True, exist_ok=False)
    runs = [(page, page, []) for page in pages]
    for page in ('appearance', 'forum', 'stage', 'screen-share', 'server-roles', 'mod-view', 'conversation', 'extensions'):
        runs.append((page+'-light-narrow', page, ['--light', '--width=560', '--height=580']))
    manifest = []
    for name, page, options in runs:
        file = output / (name + '.png')
        print('Capturing', name, flush=True)
        try:
            subprocess.run([binary, '--demo', '--page='+page, '--output='+str(file), *options], check=True, timeout=35)
            if not 32 < file.stat().st_size <= 8 * 1024 * 1024:
                raise ValueError('invalid frame size')
            with file.open('rb') as image:
                header = image.read(24)
            if header[:8] != b'\x89PNG\r\n\x1a\n' or header[12:16] != b'IHDR':
                raise ValueError('missing PNG header')
            width, height = struct.unpack('>II', header[16:24])
            if not (0 < width <= 6144 and 0 < height <= 6144):
                raise ValueError('invalid frame dimensions')
            manifest.append(dict(name=name, page=page, status='captured', pixels=[width, height]))
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            print(f'{name}: {error}', flush=True)
            manifest.append(dict(name=name, page=page, status='failed'))
        (output / 'manifest.json').write_text(json.dumps(manifest, indent=2)+'\n', encoding='utf-8')
    if any(row['status'] != 'captured' for row in manifest):
        raise SystemExit('Incomplete gallery; inspect manifest.json')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    capture(args.binary, args.output.resolve())
