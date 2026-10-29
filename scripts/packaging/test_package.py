import hashlib
import importlib.util
import io
import os
from pathlib import Path
import plistlib
import shutil
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts/packaging' / f'{name}.py')
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


class PackagingTests(unittest.TestCase):
    def test_all_payloads_contain_resources_and_platform_updaters(self):
        for system, machine, key, subfolder, runtime in [
            ('Windows', 'AMD64', 'windows-x64', 'app', 'windows/wayx-windows-x64'),
            ('Linux', 'x86_64', 'linux-x64', 'usr/lib/alacrterm', 'linux/wayx-linux-x64'),
            ('Darwin', 'arm64', 'macos-arm64', 'Alacrterm.app/Contents/MacOS', 'macos/wayx-macos-arm64'),
        ]:
            with self.subTest(system=system), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                package = module('package')
                package.ROOT = root
                release = root / 'target/release'
                release.mkdir(parents=True)
                for name in ['alacrterm', 'alacrterm.exe', 'alacrterm-update-helper.exe']:
                    (release / name).write_bytes(b'fixture')
                (root / 'crates/alacrterm').mkdir(parents=True)
                shutil.copy2(ROOT / 'crates/alacrterm/Cargo.toml', root / 'crates/alacrterm/Cargo.toml')
                shutil.copytree(ROOT / 'themes', root / 'themes')
                (root / 'plugins' / runtime / 'bin').mkdir(parents=True)
                (root / 'assets/windows/conpty').mkdir(parents=True)
                shutil.copy2(ROOT / 'assets/app-icon.ico', root / 'assets/app-icon.ico')
                framework = root / 'sparkle/Sparkle.framework'
                framework.mkdir(parents=True)
                (framework / 'fixture').write_bytes(b'framework')
                try:
                    with patch.object(package.platform, 'system', return_value=system), patch.object(package.platform, 'machine', return_value=machine), patch.object(package, 'run') as commands, patch.object(Path, 'symlink_to'), patch.dict(os.environ, {'SPARKLE_FRAMEWORK_DIR': str(framework.parent), 'SPARKLE_PUBLIC_KEY': 'public-test-key', 'APPLE_SIGNING_IDENTITY': '-'}, clear=True):
                        package.main()
                    payload = root / 'target/package' / key / subfolder
                    self.assertTrue((payload / 'plugins' / runtime / 'bin').is_dir())
                    for group, count in [('global', 21), ('terminal', 6)]:
                        self.assertEqual(len(list((payload / 'themes' / group).glob('*.json'))), count)
                    if system == 'Windows':
                        self.assertTrue((payload / 'alacrterm-update-helper.exe').is_file())
                        self.assertFalse((payload / 'app-icon.ico').exists())
                        self.assertIn(f'/DAppIcon={root / "assets/app-icon.ico"}', commands.call_args.args)
                        self.assertEqual(str(commands.call_args.args[-1]), str(root / 'scripts/packaging/windows.iss'))
                    if system == 'Linux':
                        desktop = (root / 'target/package' / key / 'usr/share/applications/alacrterm.desktop').read_text()
                        self.assertIn('Icon=alacrterm\n', desktop)
                        icon = root / 'target/package' / key / 'usr/share/icons/hicolor/256x256/apps/alacrterm.png'
                        self.assertEqual(icon.read_bytes(), package.icon_images(ROOT / 'assets/app-icon.ico')[256])
                    if system == 'Darwin':
                        app = payload.parent
                        info = plistlib.loads((app / 'Info.plist').read_bytes())
                        self.assertEqual(info['CFBundleIconFile'], 'app-icon.icns')
                        icon = (app / 'Resources/app-icon.icns').read_bytes()
                        self.assertEqual(icon[:4], b'icns')
                        self.assertEqual(int.from_bytes(icon[4:8], 'big'), len(icon))
                        self.assertIn(package.icon_images(ROOT / 'assets/app-icon.ico')[256], icon)
                        self.assertEqual(info['SUPublicEDKey'], 'public-test-key')
                        self.assertTrue(info['SUFeedURL'].endswith('/appcast.xml'))
                        self.assertTrue((app / 'Frameworks/Sparkle.framework/fixture').is_file())
                        self.assertTrue(any(args.args[0] == 'ditto' and str(args.args[-1]).endswith('.app.zip') for args in commands.call_args_list))
                finally:
                    os.chdir(ROOT)

    def test_sparkle_archive_requires_matching_digest_and_blocks_traversal(self):
        fetch = module('fetch_sparkle')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / 'sparkle.tar'
            with tarfile.open(archive, 'w') as bundle:
                info = tarfile.TarInfo('../escape')
                info.size = 3
                bundle.addfile(info, io.BytesIO(b'bad'))
            with self.assertRaises(ValueError):
                fetch.unpack(archive, root / 'output', '0' * 64)
            with self.assertRaises(tarfile.FilterError):
                fetch.unpack(archive, root / 'output', hashlib.sha256(archive.read_bytes()).hexdigest())
            self.assertFalse((root / 'escape').exists())


if __name__ == '__main__':
    unittest.main()
