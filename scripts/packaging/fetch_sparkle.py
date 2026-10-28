"""Fetch the pinned official Sparkle distribution and verify its release digest."""
import argparse
import hashlib
from pathlib import Path
import shutil
import tarfile
import tempfile
import urllib.request

VERSION = '2.10.1'
URL = f'https://github.com/sparkle-project/Sparkle/releases/download/{VERSION}/Sparkle-{VERSION}.tar.xz'
SHA256 = '1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27'


def unpack(archive, destination, expected=SHA256):
    with Path(archive).open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    if digest != expected:
        raise ValueError('Sparkle archive checksum mismatch')
    destination = Path(destination)
    destination.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive) as bundle:
        bundle.extractall(destination, filter='data')
    if not (destination / 'Sparkle.framework').is_dir():
        raise ValueError('Sparkle.framework missing from distribution')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--destination', type=Path, default=Path('target/sparkle'))
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as temporary:
        archive = Path(temporary) / 'sparkle.tar.xz'
        with urllib.request.urlopen(URL, timeout=60) as source, archive.open('wb') as output:
            shutil.copyfileobj(source, output)
        unpack(archive, args.destination)
    print(args.destination.resolve())


if __name__ == '__main__':
    main()
