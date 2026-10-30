"""Download a single trusted run's installer artifacts and validate their contents."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

import artifacts
import resolve_ci


def main():
    repo = os.environ['GH_REPO']
    run_id = os.environ['PACKAGE_RUN_ID']
    sha = os.environ['RELEASE_SHA']
    pages = resolve_ci.api(f'repos/{repo}/actions/runs/{run_id}')
    run = pages[0]
    trusted_push = (Path(run['path']).name == 'package.yml' and run['conclusion'] == 'success')
    # A reusable rebuild belongs to this release run, whose publication is not complete yet.
    own_rebuild = (run_id == os.environ['GITHUB_RUN_ID'] and Path(run['path']).name == 'release.yml')
    if (run['head_sha'] != sha or run['head_repository']['full_name'] != repo
            or run['event'] != 'push' or not (trusted_push or own_rebuild)):
        raise ValueError('Installer source is not a trusted package run at the release SHA')
    destination = Path('dist')
    if destination.exists():
        raise ValueError('dist must not exist before downloading installers')
    with tempfile.TemporaryDirectory() as temporary:
        merged = Path(temporary) / 'merged'
        merged.mkdir()
        for platform in artifacts.PLATFORMS:
            folder = Path(temporary) / platform
            subprocess.run(['gh', 'run', 'download', run_id, '--repo', repo, '--name', f'installer-{platform}', '--dir', str(folder)], check=True)
            for source in folder.iterdir():
                target = merged / source.name
                if target.exists() or source.is_symlink() or not source.is_file():
                    raise ValueError(f'Duplicate or invalid artifact file: {source.name}')
                shutil.copy2(source, target)
        artifacts.verify(merged, sha, run_id, artifacts.version())
        shutil.copytree(merged, destination)


if __name__ == '__main__':
    main()
