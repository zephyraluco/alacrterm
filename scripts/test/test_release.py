import base64
import json
import os
from pathlib import Path
import tempfile
import tomllib
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

from test_package import module


class ReleaseTests(unittest.TestCase):
    def fixture(self, dist):
        release = module('release')
        for filename in ['alacrterm-0.1.0-windows-x64-setup.exe', 'alacrterm-0.1.0-linux-x64.deb', 'alacrterm-0.1.0-macos-arm64.app.zip']:
            (dist / filename).write_bytes(b'fixture')
        (dist / 'sparkle-signature.json').write_text(json.dumps({'signature': base64.b64encode(bytes(64)).decode(), 'length': 7}), encoding='utf-8')
        return release

    def test_manifest_matches_actual_packages_and_appcast_escapes_notes(self):
        with tempfile.TemporaryDirectory() as temporary:
            dist = Path(temporary)
            release = self.fixture(dist)
            document = release.manifest(dist, '0.1.0', 'https://example.com/v0.1.0', '<invalid>& notes')
            self.assertEqual(set(document['assets']), {'windows-x64', 'linux-x64'})
            for asset in document['assets'].values():
                self.assertEqual(asset['size'], 7)
                self.assertEqual(len(asset['sha256']), 64)
            release.appcast(dist, '0.1.0', 'https://example.com/v0.1.0', '<invalid>& notes')
            item = ET.parse(dist / 'appcast.xml').find('./channel/item')
            self.assertEqual(item.findtext('description'), '<invalid>& notes')
            self.assertEqual(item.find('enclosure').get('length'), '7')
            self.assertEqual(item.findtext(f'{{{release.SPARKLE}}}version'), '0.1.0')

    def test_invalid_tag_or_stale_sparkle_signature_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            dist = Path(temporary)
            release = self.fixture(dist)
            with (release.ROOT / 'crates/alacrterm/Cargo.toml').open('rb') as source:
                current = tomllib.load(source)['package']['version']
            self.assertEqual(release.version_for_tag(f'v{current}'), current)
            major, minor, patch_version = map(int, current.split('.'))
            for tag in [current, f'v{major}.{minor}.{patch_version + 1}', f'v{current}-beta.1']:
                with self.assertRaises(ValueError):
                    release.version_for_tag(tag)
            (dist / 'alacrterm-0.1.0-macos-arm64.app.zip').write_bytes(b'changed-package')
            with self.assertRaises(ValueError):
                release.appcast(dist, '0.1.0', 'https://example.com/v0.1.0', '')

    def test_manifest_signing_verifies_embedded_key_before_publishing(self):
        release = module('release')
        document = {'version': '0.1.0', 'notes': '更新说明'}
        def signer(args, **kwargs):
            if '-S' in args:
                source = Path(args[args.index('-m') + 1])
                self.assertEqual(json.loads(source.read_text(encoding='utf-8')), document)
                Path(args[args.index('-x') + 1]).write_text('signed-fixture', encoding='utf-8')
            class Status:
                returncode = 0
            return Status()
        with patch.object(release.subprocess, 'run', side_effect=signer), patch.dict(os.environ, {'ALACRTERM_UPDATE_PUBLIC_KEYS': 'public-key'}):
            envelope = release.sign_manifest(document, 'secret-fixture', '')
        self.assertEqual(json.loads(envelope['manifest']), document)
        self.assertEqual(envelope['signature'], 'signed-fixture')


if __name__ == '__main__':
    unittest.main()
