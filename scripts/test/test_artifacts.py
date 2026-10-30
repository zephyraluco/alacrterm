import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from test_package import module


class ArtifactTests(unittest.TestCase):
    def fixture(self, folder):
        artifacts = module('artifacts')
        for platform in artifacts.PLATFORMS:
            for name in artifacts.filenames(platform, artifacts.version()):
                path = folder / name
                if name.endswith('.sha256'):
                    package = folder / name.removesuffix('.sha256')
                    path.write_text(f'{artifacts.describe(package)["sha256"]}  {package.name}\n', encoding='utf-8')
                else:
                    path.write_bytes(b'installer fixture')
            artifacts.write(folder, platform)
        return artifacts

    def test_build_identity_and_exact_artifact_set(self):
        env = {'GITHUB_SHA': 'a' * 40, 'GITHUB_RUN_ID': '12', 'GITHUB_RUN_ATTEMPT': '1',
               'ALACRTERM_UPDATE_PUBLIC_KEYS': 'public-a public-b', 'SPARKLE_PUBLIC_KEY': 'public-c'}
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, env, clear=True):
            folder = Path(temporary)
            artifacts = self.fixture(folder)
            artifacts.verify(folder, 'a' * 40, '12', artifacts.version())
            for sha, run in [('b' * 40, '12'), ('a' * 40, '13')]:
                with self.assertRaisesRegex(ValueError, 'identity mismatch'):
                    artifacts.verify(folder, sha, run, artifacts.version())
            with patch.dict(os.environ, {'SPARKLE_PUBLIC_KEY': 'changed'}):
                with self.assertRaisesRegex(ValueError, 'Public keys changed'):
                    artifacts.verify(folder, 'a' * 40, '12', artifacts.version())
            package = folder / artifacts.filenames('linux-x64', artifacts.version())[0]
            package.write_bytes(b'corrupt')
            with self.assertRaisesRegex(ValueError, 'integrity mismatch'):
                artifacts.verify(folder, 'a' * 40, '12', artifacts.version())

    def test_missing_platform_and_injected_files_are_rejected(self):
        env = {'GITHUB_SHA': 'a' * 40, 'GITHUB_RUN_ID': '12', 'GITHUB_RUN_ATTEMPT': '1'}
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, env, clear=True):
            folder = Path(temporary)
            artifacts = self.fixture(folder)
            injected = folder / 'unexpected.exe'
            injected.write_bytes(b'injected')
            with self.assertRaisesRegex(ValueError, 'Unexpected files'):
                artifacts.verify(folder, 'a' * 40, '12', artifacts.version())
            injected.unlink()
            path = folder / 'build-macos-arm64.json'
            document = json.loads(path.read_text())
            document['files'][0]['name'] = '../escape'
            path.write_text(json.dumps(document))
            with self.assertRaisesRegex(ValueError, 'file manifest'):
                artifacts.verify(folder, 'a' * 40, '12', artifacts.version())
