"""Resume draft uploads, verify remote bytes, and publish without regressing latest."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
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

    def release(self, tag):
        published = self.api('releases/tags/' + quote(tag, safe=''), optional=True)
        if published is not None:
            return published
        # The tag endpoint does not expose unpublished drafts. Authenticated listing does.
        matches = []
        page = 1
        while True:
            releases = self.api(f'releases?per_page=100&page={page}')
            matches.extend(release for release in releases if release['tag_name'] == tag)
            if len(releases) < 100:
                break
            page += 1
        if len(matches) > 1:
            raise ValueError(f'Multiple releases match {tag}; review duplicate drafts')
        return matches[0] if matches else None

    def assets(self, release):
        pages = json.loads(subprocess.check_output([
            'gh', 'api', '--paginate', '--slurp',
            f'repos/{self.repo}/releases/{release["id"]}/assets?per_page=100',
        ], text=True))
        return [item for page in pages for item in page]

    def download(self, asset, path):
        with path.open('wb') as output:
            subprocess.run(['gh', 'api', '-H', 'Accept: application/octet-stream',
                            f'repos/{self.repo}/releases/assets/{asset["id"]}'], stdout=output, check=True)

    def command(self, *args):
        subprocess.run(['gh', 'release', *args, '--repo', self.repo], check=True)


def describe(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError(f'Invalid publication file: {path.name}')
    with path.open('rb') as source:
        return {'name': path.name, 'size': path.stat().st_size,
                'sha256': hashlib.file_digest(source, 'sha256').hexdigest()}


def verify_assets(github, release, manifest):
    assets = github.assets(release)
    expected = {item['name']: item for item in manifest['files']}
    expected['release-manifest.json'] = None
    if len(assets) != len(expected) or {item['name'] for item in assets} != set(expected):
        raise ValueError('Remote release asset set is incomplete or contains unexpected files')
    with tempfile.TemporaryDirectory() as temporary:
        for asset in assets:
            path = Path(temporary) / asset['name']
            if Path(asset['name']).name != asset['name']:
                raise ValueError('Invalid remote asset name')
            github.download(asset, path)
            if asset['name'] == 'release-manifest.json':
                if json.loads(path.read_text(encoding='utf-8')) != manifest:
                    raise ValueError('Remote publication manifest mismatch')
            elif describe(path) != expected[asset['name']]:
                raise ValueError(f'Remote asset checksum mismatch: {asset["name"]}')


def verify_published(github, release, dist, tag, sha):
    assets = github.assets(release)
    matches = [a for a in assets if a['name'] == 'release-manifest.json']
    if len(matches) != 1:
        raise ValueError('Published release has no unique publication manifest; refusing to overwrite')
    with tempfile.TemporaryDirectory() as temporary:
        path = Path(temporary) / 'manifest.json'
        github.download(matches[0], path)
        manifest = json.loads(path.read_text(encoding='utf-8'))
    if manifest.get('schema') != 1 or manifest.get('tag') != tag or manifest.get('commit') != sha:
        raise ValueError('Published release identity mismatch')
    expected_names = {p.name for p in dist.iterdir()} | {'update.json', 'appcast.xml', 'sparkle-signature.json'}
    expected_names.discard('release-manifest.json')
    if {item['name'] for item in manifest['files']} != expected_names:
        raise ValueError('Published release is incomplete')
    recorded = {item['name']: item for item in manifest['files']}
    # Generated feeds contain timestamps/signatures. Check their original uploaded bytes,
    # while requiring every source installer and build manifest to match this attempt.
    for path in dist.iterdir():
        if path.name not in {'update.json', 'appcast.xml', 'sparkle-signature.json', 'release-manifest.json'}:
            if describe(path) != recorded.get(path.name):
                raise ValueError(f'Published installer differs: {path.name}; refusing to overwrite')
    verify_assets(github, release, manifest)


def publish(github, dist, tag, sha, notes, check_only=False):
    stable_version(tag)
    if github.tag_sha(tag) != sha:
        raise ValueError('Release tag moved away from the validated commit')
    existing = github.release(tag)
    if existing and not existing['draft']:
        verify_published(github, existing, dist, tag, sha)
        print(f'{tag} already published and verified; no assets modified')
        return True
    if check_only:
        return False
    paths = sorted((p for p in dist.iterdir() if p.name != 'release-manifest.json'), key=lambda p: p.name)
    manifest = {'schema': 1, 'tag': tag, 'commit': sha, 'files': [describe(p) for p in paths]}
    if not {'update.json', 'appcast.xml', 'sparkle-signature.json'}.issubset({p.name for p in paths}):
        raise ValueError('Signed update feeds are missing')
    manifest_path = dist / 'release-manifest.json'
    manifest_path.write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
    if existing is None:
        github.command('create', tag, '--verify-tag', '--target', sha, '--draft', '--notes-file', str(notes))
    else:
        github.command('edit', tag, '--notes-file', str(notes))
    existing = github.release(tag)
    if existing is None or not existing['draft']:
        raise ValueError('Expected a draft release before uploading')
    allowed = {p.name for p in paths} | {'release-manifest.json'}
    if any(a['name'] not in allowed for a in github.assets(existing)):
        raise ValueError('Draft contains unexpected attachments; review them before retrying')
    github.command('upload', tag, *(str(p) for p in paths), str(manifest_path), '--clobber')
    verify_assets(github, existing, manifest)
    # Publication is serialized by the workflow; re-read latest and the tag at the last moment.
    if github.tag_sha(tag) != sha:
        raise ValueError('Release tag changed before publication')
    latest = github.api('releases/latest', optional=True)
    make_latest = latest is None or stable_version(tag) >= stable_version(latest['tag_name'])
    github.command('edit', tag, '--draft=false', '--latest=' + str(make_latest).lower())
    return False


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    done = publish(GitHub(os.environ['GH_REPO']), Path('dist'), os.environ['RELEASE_TAG'],
                   os.environ['RELEASE_SHA'], Path(os.environ['RUNNER_TEMP']) / 'release-notes.md', args.check)
    if args.check:
        with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as output:
            output.write(f'already_published={str(done).lower()}\n')


if __name__ == '__main__':
    main()
