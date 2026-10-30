"""Upload one complete release, then publish without regressing latest."""
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import quote


def stable_version(tag):
    match = re.fullmatch(r'v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)', tag)
    if not match:
        raise ValueError(f'Cannot compare non-stable release tag: {tag}')
    return tuple(map(int, match.groups()))


class GitHub:
    def __init__(self, repo):
        self.repo = repo

    def api(self, path, optional=False):
        result = subprocess.run(['gh', 'api', f'repos/{self.repo}/{path}'], text=True, capture_output=True)
        if result.returncode:
            if optional and '(HTTP 404)' in result.stderr:
                return None
            raise RuntimeError(f'GitHub API failed for {path}: {result.stderr}')
        return json.loads(result.stdout)

    def tag_sha(self, tag):
        item = self.api('git/ref/tags/' + quote(tag, safe=''))['object']
        for _ in range(16):
            if item['type'] == 'commit':
                return item['sha']
            if item['type'] != 'tag':
                break
            item = self.api('git/tags/' + item['sha'])['object']
        raise ValueError('Tag does not resolve to a commit')

    def command(self, *args):
        subprocess.run(['gh', 'release', *args, '--repo', self.repo], check=True)


def publish(github, dist, tag, sha, notes):
    stable_version(tag)
    if github.tag_sha(tag) != sha:
        raise ValueError('Release tag moved away from the validated commit')
    paths = sorted(dist.iterdir(), key=lambda path: path.name)
    if not {'update.json', 'appcast.xml', 'sparkle-signature.json'}.issubset({p.name for p in paths}):
        raise ValueError('Signed update feeds are missing')
    if any(path.is_symlink() or not path.is_file() for path in paths):
        raise ValueError('Publication contains non-regular files')
    latest = github.api('releases/latest', optional=True)
    make_latest = latest is None or stable_version(tag) >= stable_version(latest['tag_name'])
    # gh uploads every attachment as part of create. Existing releases fail rather
    # than being resumed or overwritten; drafts keep partial uploads out of public view.
    github.command('create', tag, *(str(path) for path in paths), '--verify-tag',
                   '--target', sha, '--draft', '--notes-file', str(notes))
    if github.tag_sha(tag) != sha:
        raise ValueError('Release tag changed before publication')
    github.command('edit', tag, '--draft=false', '--latest=' + str(make_latest).lower())


def main():
    publish(GitHub(os.environ['GH_REPO']), Path('dist'), os.environ['RELEASE_TAG'],
            os.environ['RELEASE_SHA'], Path(os.environ['RUNNER_TEMP']) / 'release-notes.md')


if __name__ == '__main__':
    main()
