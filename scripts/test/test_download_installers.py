import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

from test_package import ROOT, module


class DownloadTests(unittest.TestCase):
    def test_external_run_cannot_use_release_rebuild_exception(self):
        with patch.object(sys, 'path', [str(ROOT / 'scripts/packaging'), *sys.path]):
            download = module('download_installers')
        env = {'GH_REPO': 'owner/repo', 'RELEASE_SHA': 'sha', 'PACKAGE_RUN_ID': '99', 'GITHUB_RUN_ID': '100'}
        run = {'head_sha': 'sha', 'head_repository': {'full_name': 'owner/repo'},
               'event': 'push', 'path': '.github/workflows/release.yml', 'conclusion': None}
        with patch.dict(os.environ, env), patch.object(download.resolve_ci, 'api', return_value=[run]):
            with self.assertRaisesRegex(ValueError, 'trusted package run'):
                download.main()

    def test_named_artifacts_merge_only_after_identity_verification(self):
        with patch.object(sys, 'path', [str(ROOT / 'scripts/packaging'), *sys.path]):
            download = module('download_installers')
        env = {'GH_REPO': 'owner/repo', 'RELEASE_SHA': 'a' * 40, 'PACKAGE_RUN_ID': '99',
               'GITHUB_RUN_ID': '100', 'GITHUB_SHA': 'a' * 40, 'GITHUB_RUN_ATTEMPT': '1'}
        run = {'head_sha': 'a' * 40, 'head_repository': {'full_name': 'owner/repo'},
               'event': 'push', 'path': '.github/workflows/package.yml', 'conclusion': 'success'}
        def gh_download(args, **kwargs):
            platform = args[args.index('--name') + 1].removeprefix('installer-')
            folder = Path(args[args.index('--dir') + 1])
            folder.mkdir()
            for name in download.artifacts.filenames(platform, download.artifacts.version()):
                path = folder / name
                if name.endswith('.sha256'):
                    package = folder / name.removesuffix('.sha256')
                    path.write_text(f'{download.artifacts.describe(package)["sha256"]}  {package.name}\n')
                else:
                    path.write_bytes(b'package')
            with patch.dict(os.environ, {'GITHUB_RUN_ID': '99'}):
                download.artifacts.write(folder, platform)
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, env):
            try:
                os.chdir(temporary)
                with patch.object(download.resolve_ci, 'api', return_value=[run]), patch.object(download.subprocess, 'run', side_effect=gh_download):
                    download.main()
                download.artifacts.verify(Path('dist'), 'a' * 40, '99', download.artifacts.version())
            finally:
                os.chdir(ROOT)
