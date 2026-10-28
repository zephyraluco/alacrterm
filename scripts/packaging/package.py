"""Build a native installer from target/release, without modifying the checkout."""
import hashlib
import os
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[2]


def run(*args):
    subprocess.run([str(arg) for arg in args], check=True)


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
        control = work / 'DEBIAN'
        control.mkdir()
        (control / 'control').write_text(f'Package: alacrterm\nVersion: {version}\nArchitecture: amd64\nMaintainer: Alacrterm contributors\nDepends: libfontconfig1, libfreetype6, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libegl1, libvulkan1, libxcb1, libxcb-shape0, libxcb-xfixes0, libasound2t64, libssl3t64\nDescription: Terminal and SSH client\n', encoding='utf-8')
        launch = work / 'usr/bin'
        launch.mkdir(parents=True)
        (launch / 'alacrterm').symlink_to('../lib/alacrterm/alacrterm')
        desktop = work / 'usr/share/applications'
        desktop.mkdir(parents=True)
        (desktop / 'alacrterm.desktop').write_text('[Desktop Entry]\nType=Application\nName=Alacrterm\nExec=alacrterm\nTerminal=false\nCategories=System;TerminalEmulator;\n', encoding='utf-8')
        for directory in [work, *[path for path in work.rglob('*') if path.is_dir()]]:
            directory.chmod(0o755)
        run('dpkg-deb', '--root-owner-group', '--build', work, dist / f'{name}.deb')
    elif system == 'Darwin':
        with (app / 'Contents/Info.plist').open('wb') as file:
            plistlib.dump({'CFBundleExecutable': binary, 'CFBundleIdentifier': 'com.alacrterm.app', 'CFBundleName': 'Alacrterm', 'CFBundlePackageType': 'APPL', 'CFBundleShortVersionString': version, 'CFBundleVersion': version, 'NSHighResolutionCapable': True}, file)
        run('codesign', '--force', '--deep', '--sign', '-', app)
        (work / 'Applications').symlink_to('/Applications')
        run('hdiutil', 'create', '-volname', 'Alacrterm', '-srcfolder', work, '-ov', '-format', 'UDZO', dist / f'{name}.dmg')
    else:
        for file in (ROOT / 'assets/windows/conpty').iterdir():
            if file.is_file():
                shutil.copy2(file, payload / file.name)
        compiler = Path(os.environ.get('ProgramFiles(x86)', r'C:\Program Files (x86)')) / 'Inno Setup 6/ISCC.exe'
        run(compiler, f'/DAppVersion={version}', f'/DSourceDir={payload}', f'/DOutputDir={dist}', f'/DOutputName={name}-setup', ROOT / 'scripts/packaging/windows.iss')
    for file in dist.iterdir():
        if file.suffix in {'.deb', '.dmg', '.exe'}:
            digest = hashlib.file_digest(file.open('rb'), 'sha256').hexdigest()
            file.with_name(file.name + '.sha256').write_text(f'{digest}  {file.name}\n', encoding='utf-8')


if __name__ == '__main__':
    main()
