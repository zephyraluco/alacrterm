"""Describe and verify immutable installer artifacts before release signing."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[2]
PLATFORMS = ('linux-x64', 'windows-x64', 'macos-arm64')


def version():
    with (ROOT / 'crates/alacrterm/Cargo.toml').open('rb') as source:
        return tomllib.load(source)['package']['version']


def filenames(platform, release_version):
    prefix = f'alacrterm-{release_version}-{platform}'
    endings = {'linux-x64': ['.deb'], 'windows-x64': ['-setup.exe'], 'macos-arm64': ['.dmg', '.app.zip']}
    packages = [prefix + suffix for suffix in endings[platform]]
    return packages + [name + '.sha256' for name in packages]


def keys():
    return {
        'minisign': hashlib.sha256('\n'.join(sorted(os.environ.get('ALACRTERM_UPDATE_PUBLIC_KEYS', '').split())).encode()).hexdigest(),
        'sparkle': hashlib.sha256(os.environ.get('SPARKLE_PUBLIC_KEY', '').strip().encode()).hexdigest(),
    }


def describe(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError(f'Missing or non-regular artifact: {path.name}')
    with path.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
    return {'name': path.name, 'size': path.stat().st_size, 'sha256': digest}


def write(dist, platform):
    sha = os.environ['GITHUB_SHA']
    if not re.fullmatch(r'[0-9a-f]{40}', sha):
        raise ValueError('Expected full commit SHA')
    document = {
        'schema': 1, 'commit': sha, 'version': version(), 'platform': platform,
        'run_id': os.environ['GITHUB_RUN_ID'], 'run_attempt': os.environ['GITHUB_RUN_ATTEMPT'],
        'public_keys': keys(), 'files': [describe(dist / name) for name in filenames(platform, version())],
    }
    (dist / f'build-{platform}.json').write_text(json.dumps(document, indent=2) + '\n', encoding='utf-8')


def verify(dist, sha, run_id, release_version):
    allowed = set()
    attempts = set()
    for platform in PLATFORMS:
        metadata = f'build-{platform}.json'
        allowed.add(metadata)
        document = json.loads((dist / metadata).read_text(encoding='utf-8'))
        expected = {'schema': 1, 'commit': sha, 'run_id': str(run_id), 'version': release_version, 'platform': platform}
        if any(document.get(key) != value for key, value in expected.items()):
            raise ValueError(f'Build identity mismatch: {platform}')
        if document.get('public_keys') != keys():
            raise ValueError(f'Public keys changed since build: {platform}; rebuild the packages')
        attempts.add(document.get('run_attempt'))
        expected_files = filenames(platform, release_version)
        files = document.get('files', [])
        if sorted(item['name'] for item in files) != sorted(expected_files):
            raise ValueError(f'Incomplete or unexpected file manifest: {platform}')
        for item in files:
            if describe(dist / item['name']) != item:
                raise ValueError(f'Artifact integrity mismatch: {item["name"]}')
            allowed.add(item['name'])
            if item['name'].endswith('.sha256'):
                package = item['name'].removesuffix('.sha256')
                expected_sum = f'{describe(dist / package)["sha256"]}  {package}\n'
                if (dist / item['name']).read_text(encoding='utf-8') != expected_sum:
                    raise ValueError(f'Invalid checksum file: {item["name"]}')
    # Re-running failed jobs can legitimately produce artifacts from different attempts.
    if any(not isinstance(attempt, str) or not attempt.isdecimal() or int(attempt) < 1 for attempt in attempts):
        raise ValueError('Invalid run attempt')
    if {p.name for p in dist.iterdir()} != allowed:
        raise ValueError('Unexpected files in installer artifact set')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('command', choices=['write', 'verify'])
    parser.add_argument('--dist', type=Path, default=Path('dist'))
    parser.add_argument('--platform', choices=PLATFORMS)
    args = parser.parse_args()
    if args.command == 'write':
        if not args.platform:
            parser.error('--platform is required for write')
        write(args.dist, args.platform)
    else:
        verify(args.dist, os.environ['RELEASE_SHA'], os.environ['PACKAGE_RUN_ID'], version())


if __name__ == '__main__':
    main()
