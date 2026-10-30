import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from test_package import module


class FakeGitHub:
    def __init__(self, latest='v0.0.9'):
        self.current = None
        self.files = {}
        self.commands = []
        self.latest = latest
        self.sha = 'a' * 40
        self.interrupt = False

    def tag_sha(self, tag):
        return self.sha

    def release(self, tag):
        return self.current

    def assets(self, release):
        return [{'name': name, 'id': name} for name in self.files]

    def download(self, asset, path):
        path.write_bytes(self.files[asset['id']])

    def api(self, path, optional=False):
        assert path == 'releases/latest'
        return {'tag_name': self.latest} if self.latest else None

    def command(self, *args):
        self.commands.append(args)
        if args[0] == 'create':
            self.current = {'id': 1, 'draft': True}
        elif args[0] == 'upload':
            for filename in args[2:]:
                if filename.startswith('--'):
                    continue
                path = Path(filename)
                self.files[path.name] = path.read_bytes()
                if self.interrupt:
                    self.interrupt = False
                    raise RuntimeError('upload interrupted')
        elif '--draft=false' in args:
            self.current['draft'] = False


class PublicationTests(unittest.TestCase):
    def test_draft_lookup_uses_authenticated_paginated_list_when_tag_is_not_found(self):
        publisher = module('publish_release')
        github = publisher.GitHub('owner/repo')
        draft = {'id': 12, 'tag_name': 'v0.1.1', 'draft': True}
        first_page = [{'id': i, 'tag_name': f'v0.0.{i}', 'draft': False} for i in range(100)]
        with patch.object(github, 'api', side_effect=[None, first_page, [draft]]) as api:
            self.assertEqual(github.release('v0.1.1'), draft)
            self.assertEqual(api.call_args.args[0], 'releases?per_page=100&page=2')
        with patch.object(github, 'api', side_effect=[None, []]):
            self.assertIsNone(github.release('v9.9.9'))
        with patch.object(github, 'api', return_value=draft) as api:
            self.assertEqual(github.release('v0.1.1'), draft)
            api.assert_called_once()
        with patch.object(github, 'api', side_effect=[None, [draft, draft]]):
            with self.assertRaisesRegex(ValueError, 'Multiple releases'):
                github.release('v0.1.1')

    def fixture(self, directory):
        for name in ['installer.exe', 'build-windows-x64.json', 'update.json', 'appcast.xml', 'sparkle-signature.json']:
            (directory / name).write_text('fixture', encoding='utf-8')
        return module('publish_release')

    def test_draft_upload_resumes_and_published_retry_never_overwrites(self):
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            publisher = self.fixture(folder)
            github = FakeGitHub()
            github.interrupt = True
            with self.assertRaisesRegex(RuntimeError, 'interrupted'):
                publisher.publish(github, folder, 'v0.1.0', github.sha, folder / 'notes')
            self.assertTrue(github.current['draft'])
            publisher.publish(github, folder, 'v0.1.0', github.sha, folder / 'notes')
            self.assertFalse(github.current['draft'])
            self.assertEqual(sum(c[0] == 'create' for c in github.commands), 1)
            count = len(github.commands)
            # Feed regeneration timestamps must not make an immutable release retry overwrite assets.
            (folder / 'appcast.xml').write_text('new generated timestamp')
            self.assertTrue(publisher.publish(github, folder, 'v0.1.0', github.sha, folder / 'notes'))
            self.assertEqual(len(github.commands), count)
            (folder / 'installer.exe').write_text('different installer')
            with self.assertRaisesRegex(ValueError, 'refusing to overwrite'):
                publisher.publish(github, folder, 'v0.1.0', github.sha, folder / 'notes')
            self.assertEqual(len(github.commands), count)

    def test_latest_never_regresses_and_versions_are_numeric(self):
        publisher = module('publish_release')
        self.assertGreater(publisher.stable_version('v0.10.0'), publisher.stable_version('v0.9.0'))
        for latest, flag in [('v0.2.0', '--latest=false'), ('v0.0.9', '--latest=true'), (None, '--latest=true')]:
            with tempfile.TemporaryDirectory() as temporary:
                folder = Path(temporary)
                self.fixture(folder)
                github = FakeGitHub(latest)
                publisher.publish(github, folder, 'v0.1.0', github.sha, folder / 'notes')
                self.assertIn(flag, github.commands[-1])

    def test_moved_tag_and_corrupt_remote_assets_block_publication(self):
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            publisher = self.fixture(folder)
            github = FakeGitHub()
            with self.assertRaisesRegex(ValueError, 'tag moved'):
                publisher.publish(github, folder, 'v0.1.0', 'b' * 40, folder / 'notes')
            self.assertEqual(github.commands, [])
            publisher.publish(github, folder, 'v0.1.0', github.sha, folder / 'notes')
            github.files['installer.exe'] = b'corrupt remote file'
            with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                publisher.publish(github, folder, 'v0.1.0', github.sha, folder / 'notes')
            del github.files['appcast.xml']
            with self.assertRaisesRegex(ValueError, 'asset set'):
                publisher.publish(github, folder, 'v0.1.0', github.sha, folder / 'notes')

    def test_unknown_latest_version_and_unexpected_draft_attachments_fail_safely(self):
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            publisher = self.fixture(folder)
            github = FakeGitHub('unparseable')
            with self.assertRaisesRegex(ValueError, 'Cannot compare'):
                publisher.publish(github, folder, 'v0.1.0', github.sha, folder / 'notes')
            self.assertTrue(github.current['draft'])
            github.files['foreign.exe'] = b'unknown'
            with self.assertRaisesRegex(ValueError, 'unexpected attachments'):
                publisher.publish(github, folder, 'v0.1.0', github.sha, folder / 'notes')
            self.assertTrue(github.current['draft'])
