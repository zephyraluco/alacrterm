import base64
import os
from pathlib import Path
import runpy
import tempfile
import unittest
from unittest.mock import patch

from test_package import ROOT


class WorkflowScriptTests(unittest.TestCase):
    def test_release_validation_accepts_complete_configuration_and_rejects_missing(self):
        required = [
            'ALACRTERM_UPDATE_PUBLIC_KEYS', 'SPARKLE_PUBLIC_KEY',
            'MINISIGN_SECRET_KEY', 'SPARKLE_PRIVATE_KEY',
            'APPLE_CERTIFICATE_P12', 'APPLE_CERTIFICATE_PASSWORD',
            'APPLE_SIGNING_IDENTITY', 'APPLE_API_KEY_P8',
            'APPLE_API_KEY_ID', 'APPLE_API_ISSUER',
        ]
        env = dict.fromkeys(required, 'fixture')
        env['RELEASE_TAG'] = 'v0.1.0'
        script = ROOT / 'scripts/packaging/validate_release.py'
        with patch.dict(os.environ, env, clear=True):
            runpy.run_path(str(script), run_name='__main__')
            del os.environ['APPLE_API_KEY_P8']
            with self.assertRaisesRegex(SystemExit, 'Missing release configuration: APPLE_API_KEY_P8'):
                runpy.run_path(str(script), run_name='__main__')

    def test_apple_credentials_are_decoded_and_exported(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            exports = root / 'github-env'
            env = {
                'RUNNER_TEMP': temporary,
                'GITHUB_ENV': str(exports),
                'APPLE_CERTIFICATE_P12': base64.b64encode(b'certificate fixture').decode(),
                'APPLE_API_KEY_P8': base64.b64encode(b'notary fixture').decode(),
                'APPLE_SIGNING_IDENTITY': 'identity fixture',
                'APPLE_NOTARY_KEY_ID': 'key fixture',
                'APPLE_NOTARY_ISSUER': 'issuer fixture',
            }
            with patch.dict(os.environ, env, clear=True):
                runpy.run_path(str(ROOT / 'scripts/packaging/prepare_apple_signing.py'))
            self.assertEqual((root / 'certificate.p12').read_bytes(), b'certificate fixture')
            self.assertEqual((root / 'notary.p8').read_bytes(), b'notary fixture')
            self.assertIn(f'APPLE_NOTARY_KEY={root / "notary.p8"}\n', exports.read_text())
            if os.name != 'nt':
                self.assertEqual((root / 'certificate.p12').stat().st_mode & 0o777, 0o600)

    def test_apple_exports_reject_newlines(self):
        with tempfile.TemporaryDirectory() as temporary:
            env = {
                'RUNNER_TEMP': temporary,
                'GITHUB_ENV': str(Path(temporary) / 'github-env'),
                'APPLE_CERTIFICATE_P12': '',
                'APPLE_API_KEY_P8': '',
                'APPLE_SIGNING_IDENTITY': 'invalid\nvalue',
            }
            with patch.dict(os.environ, env, clear=True):
                with self.assertRaisesRegex(ValueError, 'Invalid signing configuration'):
                    runpy.run_path(str(ROOT / 'scripts/packaging/prepare_apple_signing.py'))
