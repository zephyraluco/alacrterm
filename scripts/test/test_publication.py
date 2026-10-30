from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock

from test_package import module


class PublicationTests(unittest.TestCase):
    def fixture(self, folder, latest='v0.0.9'):
        for name in ['installer.exe', 'update.json', 'appcast.xml', 'sparkle-signature.json']:
            (folder / name).write_text('fixture', encoding='utf-8')
        github = Mock()
        github.tag_sha.return_value = 'a' * 40
        github.api.return_value = {'tag_name': latest} if latest else None
        return module('publish_release'), github

    def test_complete_upload_precedes_publication_without_recovery_or_remote_downloads(self):
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            publisher, github = self.fixture(folder)
            publisher.publish(github, folder, 'v0.1.1', 'a' * 40, folder / 'notes')
            create, publish = github.command.call_args_list
            self.assertEqual(create.args[:2], ('create', 'v0.1.1'))
            for path in folder.iterdir():
                self.assertIn(str(path), create.args)
            self.assertIn('--draft', create.args)
            self.assertEqual(publish.args, ('edit', 'v0.1.1', '--draft=false', '--latest=true'))
            github.api.assert_called_once_with('releases/latest', optional=True)
            github.release.assert_not_called()
            github.download.assert_not_called()
            self.assertFalse((folder / 'release-manifest.json').exists())

    def test_latest_is_compared_numerically(self):
        publisher = module('publish_release')
        self.assertGreater(publisher.stable_version('v0.10.0'), publisher.stable_version('v0.9.0'))
        for latest, flag in [('v0.2.0', '--latest=false'), ('v0.0.9', '--latest=true'), (None, '--latest=true')]:
            with tempfile.TemporaryDirectory() as temporary:
                folder = Path(temporary)
                publisher, github = self.fixture(folder, latest)
                publisher.publish(github, folder, 'v0.1.1', 'a' * 40, folder / 'notes')
                self.assertIn(flag, github.command.call_args.args)

    def test_upload_failure_or_moved_tag_never_publishes(self):
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            publisher, github = self.fixture(folder)
            github.command.side_effect = RuntimeError('upload failed or release already exists')
            with self.assertRaisesRegex(RuntimeError, 'upload failed'):
                publisher.publish(github, folder, 'v0.1.1', 'a' * 40, folder / 'notes')
            self.assertEqual(github.command.call_count, 1)
            github.command.reset_mock(side_effect=True)
            github.tag_sha.side_effect = ['a' * 40, 'b' * 40]
            with self.assertRaisesRegex(ValueError, 'changed before publication'):
                publisher.publish(github, folder, 'v0.1.1', 'a' * 40, folder / 'notes')
            self.assertEqual(github.command.call_count, 1)
