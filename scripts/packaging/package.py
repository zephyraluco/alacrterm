"""Build a native installer from target/release, without modifying the checkout."""
import hashlib
import os
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import struct
import tomllib

ROOT = Path(__file__).resolve().parents[2]


def run(*args):
    subprocess.run([str(arg) for arg in args], check=True)


def normalize_permissions(root):
    for path in [root, *root.rglob('*')]:
        if path.is_symlink():
            continue
        if path.is_dir():
            path.chmod(0o755)
        elif path.is_file():
            path.chmod(0o755 if path.stat().st_mode & 0o111 else 0o644)


def icon_images(source):
    """Reuse the ICO's original PNG frames; no image-conversion dependency is needed."""
    data = source.read_bytes()
    reserved, kind, count = struct.unpack_from('<HHH', data)
    if reserved != 0 or kind != 1:
        raise ValueError('Expected an ICO application icon')
    images = {}
    for index in range(count):
        width, height, _, _, _, _, length, offset = struct.unpack_from('<BBBBHHII', data, 6 + index * 16)
        width, height = width or 256, height or 256
        frame = data[offset:offset + length]
        if width == height and frame.startswith(b'\x89PNG\r\n\x1a\n'):
            images[width] = frame
    if not {128, 256}.issubset(images):
        raise ValueError('Application ICO must contain 128px and 256px PNG images')
    return images


def write_icns(source, destination):
    images = icon_images(source)
    chunks = b''.join(kind + struct.pack('>I', len(images[size]) + 8) + images[size]
                      for size, kind in [(128, b'ic07'), (256, b'ic08')])
    destination.write_bytes(b'icns' + struct.pack('>I', len(chunks) + 8) + chunks)


def main():
    os.chdir(ROOT)
    with (ROOT / 'crates/alacrterm/Cargo.toml').open('rb') as manifest:
        version = tomllib.load(manifest)['package']['version']
    system = platform.system()
    key = {'Linux': 'linux-x64', 'Darwin': 'macos-arm64', 'Windows': 'windows-x64'}[system]
    expected = 'arm64' if system == 'Darwin' else 'amd64'
    machine = platform.machine().lower()
    if machine not in ({'arm64', 'aarch64'} if expected == 'arm64' else {'amd64', 'x86_64'}):
        raise RuntimeError(f'Unsupported architecture: {machine}')
    work = ROOT / 'target/package' / key
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir(parents=True)
    dist = ROOT / 'dist'
    dist.mkdir(exist_ok=True)
    name = f'alacrterm-{version}-{key}'
    if system == 'Darwin':
        app = work / 'Alacrterm.app'
        payload = app / 'Contents/MacOS'
    elif system == 'Linux':
        payload = work / 'usr/lib/alacrterm'
    else:
        payload = work / 'app'
    payload.mkdir(parents=True)
    binary = 'alacrterm.exe' if system == 'Windows' else 'alacrterm'
    shutil.copy2(ROOT / 'target/release' / binary, payload / binary)
    shutil.copytree(ROOT / 'themes', payload / 'themes')
    runtime = {'Linux': 'linux/wayx-linux-x64', 'Darwin': 'macos/wayx-macos-arm64', 'Windows': 'windows/wayx-windows-x64'}[system]
    shutil.copytree(ROOT / 'plugins' / runtime, payload / 'plugins' / runtime)
    if system == 'Linux':
        for size, image in icon_images(ROOT / 'assets/app-icon.ico').items():
            icons = work / f'usr/share/icons/hicolor/{size}x{size}/apps'
            icons.mkdir(parents=True)
            (icons / 'alacrterm.png').write_bytes(image)
        control = work / 'DEBIAN'
        control.mkdir()
        (control / 'control').write_text(f'Package: alacrterm\nVersion: {version}\nArchitecture: amd64\nMaintainer: Alacrterm contributors\nDepends: libfontconfig1, libfreetype6, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libegl1, libvulkan1, libxcb1, libxcb-shape0, libxcb-xfixes0, libasound2t64, libssl3t64\nDescription: Terminal and SSH client\n', encoding='utf-8')
        launch = work / 'usr/bin'
        launch.mkdir(parents=True)
        (launch / 'alacrterm').symlink_to('../lib/alacrterm/alacrterm')
        desktop = work / 'usr/share/applications'
        desktop.mkdir(parents=True)
        (desktop / 'alacrterm.desktop').write_text('[Desktop Entry]\nType=Application\nName=Alacrterm\nExec=alacrterm\nIcon=alacrterm\nStartupWMClass=alacrterm\nTerminal=false\nCategories=System;TerminalEmulator;\n', encoding='utf-8')
        normalize_permissions(work)
        run('dpkg-deb', '--root-owner-group', '--build', work, dist / f'{name}.deb')
    elif system == 'Darwin':
        resources = app / 'Contents/Resources'
        resources.mkdir()
        write_icns(ROOT / 'assets/app-icon.ico', resources / 'app-icon.icns')
        framework_dir = Path(os.environ['SPARKLE_FRAMEWORK_DIR'])
        frameworks = app / 'Contents/Frameworks'
        frameworks.mkdir()
        shutil.copytree(framework_dir / 'Sparkle.framework', frameworks / 'Sparkle.framework', symlinks=True)
        info = {'CFBundleExecutable': binary, 'CFBundleIdentifier': 'com.alacrterm.app', 'CFBundleName': 'Alacrterm', 'CFBundleIconFile': 'app-icon.icns', 'CFBundlePackageType': 'APPL', 'CFBundleShortVersionString': version, 'CFBundleVersion': version, 'LSMinimumSystemVersion': '12.0', 'NSHighResolutionCapable': True, 'SUFeedURL': 'https://github.com/zephyraluco/alacrterm/releases/latest/download/appcast.xml', 'SUEnableAutomaticChecks': True, 'SUAutomaticallyUpdate': False}
        public_key = os.environ.get('SPARKLE_PUBLIC_KEY')
        if public_key:
            info['SUPublicEDKey'] = public_key
        with (app / 'Contents/Info.plist').open('wb') as file:
            plistlib.dump(info, file)
        normalize_permissions(app)
        identity = os.environ.get('APPLE_SIGNING_IDENTITY', '-')
        if identity == '-':
            run('codesign', '--force', '--deep', '--sign', '-', app)
        else:
            framework = frameworks / 'Sparkle.framework'
            for executable in app.rglob('*'):
                if executable.is_file() and not executable.is_symlink():
                    description = subprocess.check_output(['file', '-b', str(executable)], text=True)
                    if 'Mach-O' in description:
                        signing = ['codesign', '--force', '--options', 'runtime', '--timestamp', '--sign', identity]
                        if framework in executable.parents:
                            signing.append('--preserve-metadata=identifier,entitlements')
                        run(*signing, executable)
            nested = [*framework.rglob('*.xpc'), *framework.rglob('*.app')]
            for bundle in sorted(nested, key=lambda path: len(path.parts), reverse=True):
                if not bundle.is_symlink():
                    run('codesign', '--force', '--options', 'runtime', '--timestamp', '--preserve-metadata=identifier,entitlements', '--sign', identity, bundle)
            # Autoupdate is a standalone helper executable in Sparkle's framework.
            autoupdate = framework / 'Versions/B/Autoupdate'
            if autoupdate.is_file():
                run('codesign', '--force', '--options', 'runtime', '--timestamp', '--sign', identity, autoupdate)
            run('codesign', '--force', '--options', 'runtime', '--timestamp', '--sign', identity, framework)
            run('codesign', '--force', '--options', 'runtime', '--timestamp', '--sign', identity, app)
        if os.environ.get('APPLE_NOTARY_KEY'):
            notarization = work / 'notarize.zip'
            run('ditto', '-c', '-k', '--keepParent', app, notarization)
            run('xcrun', 'notarytool', 'submit', notarization, '--key', os.environ['APPLE_NOTARY_KEY'], '--key-id', os.environ['APPLE_NOTARY_KEY_ID'], '--issuer', os.environ['APPLE_NOTARY_ISSUER'], '--wait')
            run('xcrun', 'stapler', 'staple', app)
            notarization.unlink()
        run('ditto', '-c', '-k', '--keepParent', app, dist / f'{name}.app.zip')
        (work / 'Applications').symlink_to('/Applications')
        run('hdiutil', 'create', '-volname', 'Alacrterm', '-srcfolder', work, '-ov', '-format', 'UDZO', dist / f'{name}.dmg')
    else:
        shutil.copy2(ROOT / 'target/release/alacrterm-update-helper.exe', payload / 'alacrterm-update-helper.exe')
        for file in (ROOT / 'assets/windows/conpty').iterdir():
            if file.is_file():
                shutil.copy2(file, payload / file.name)
        compiler = Path(os.environ.get('ProgramFiles(x86)', r'C:\Program Files (x86)')) / 'Inno Setup 6/ISCC.exe'
        run(compiler, f'/DAppVersion={version}', f'/DSourceDir={payload}', f'/DAppIcon={ROOT / "assets/app-icon.ico"}', f'/DOutputDir={dist}', f'/DOutputName={name}-setup', ROOT / 'scripts/packaging/windows.iss')
    for file in dist.iterdir():
        if file.suffix in {'.deb', '.dmg', '.exe', '.zip'}:
            digest = hashlib.file_digest(file.open('rb'), 'sha256').hexdigest()
            file.with_name(file.name + '.sha256').write_text(f'{digest}  {file.name}\n', encoding='utf-8')


if __name__ == '__main__':
    main()
