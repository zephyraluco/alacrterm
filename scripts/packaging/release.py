"""Create authenticated update metadata after all installers have been uploaded as CI artifacts."""
import argparse
import base64
from datetime import datetime, timezone
import email.utils
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
SPARKLE = 'http://www.andymatuschak.org/xml-namespaces/sparkle'
ET.register_namespace('sparkle', SPARKLE)


def version_for_tag(tag):
    with (ROOT / 'crates/alacrterm/Cargo.toml').open('rb') as source:
        version = tomllib.load(source)['package']['version']
    if tag != f'v{version}' or not re.fullmatch(r'\d+\.\d+\.\d+', version):
        raise ValueError('Release tag must match the stable Cargo version: v<major>.<minor>.<patch>')
    return version


def manifest(dist, version, base, notes):
    assets = {}
    for platform, filename, kind in [
        ('windows-x64', f'alacrterm-{version}-windows-x64-setup.exe', 'exe'),
        ('linux-x64', f'alacrterm-{version}-linux-x64.deb', 'deb'),
    ]:
        path = dist / filename
        with path.open('rb') as source:
            digest = hashlib.file_digest(source, 'sha256').hexdigest()
        assets[platform] = {'url': f'{base}/{filename}', 'size': path.stat().st_size, 'sha256': digest, 'kind': kind}
    return {'schema': 1, 'channel': 'stable', 'version': version, 'minimum_version': None, 'notes': notes, 'assets': assets}


def appcast(dist, version, base, notes):
    filename = f'alacrterm-{version}-macos-arm64.app.zip'
    package = dist / filename
    signature = json.loads((dist / 'sparkle-signature.json').read_text(encoding='utf-8'))
    # The signature is produced on macOS from the final app bundle ZIP.
    if signature['length'] != package.stat().st_size or len(base64.b64decode(signature['signature'], validate=True)) != 64:
        raise ValueError('Sparkle signature metadata does not match the upgrade package')
    rss = ET.Element('rss', {'version': '2.0'})
    channel = ET.SubElement(rss, 'channel')
    ET.SubElement(channel, 'title').text = 'Alacrterm updates'
    item = ET.SubElement(channel, 'item')
    ET.SubElement(item, 'title').text = f'Alacrterm {version}'
    ET.SubElement(item, 'pubDate').text = email.utils.format_datetime(datetime.now(timezone.utc))
    ET.SubElement(item, 'description').text = notes
    ET.SubElement(item, f'{{{SPARKLE}}}version').text = version
    ET.SubElement(item, f'{{{SPARKLE}}}shortVersionString').text = version
    ET.SubElement(item, f'{{{SPARKLE}}}minimumSystemVersion').text = '12.0'
    ET.SubElement(item, 'enclosure', {'url': f'{base}/{filename}', 'length': str(package.stat().st_size), 'type': 'application/octet-stream', f'{{{SPARKLE}}}edSignature': signature['signature'], f'{{{SPARKLE}}}os': 'macos', f'{{{SPARKLE}}}cpuArchitecture': 'arm64'})
    ET.ElementTree(rss).write(dist / 'appcast.xml', encoding='utf-8', xml_declaration=True)


def sign_manifest(document, key, password):
    text = json.dumps(document, ensure_ascii=False, separators=(',', ':'))
    with tempfile.TemporaryDirectory() as temporary:
        folder = Path(temporary)
        secret = folder / 'minisign.key'
        secret.write_text(key, encoding='utf-8')
        secret.chmod(0o600)
        source = folder / 'manifest.json'
        source.write_text(text, encoding='utf-8')
        signature = folder / 'manifest.minisig'
        subprocess.run(['minisign', '-S', '-s', str(secret), '-m', str(source), '-x', str(signature), '-t', f"alacrterm-manifest stable {document['version']}"], input=password + '\n', text=True, check=True, capture_output=True)
        public_keys = os.environ['ALACRTERM_UPDATE_PUBLIC_KEYS'].split()
        verified = any(subprocess.run(['minisign', '-V', '-P', public, '-m', str(source), '-x', str(signature)], capture_output=True).returncode == 0 for public in public_keys)
        if not verified:
            raise ValueError('Signing key does not match any embedded update public key')
        return {'manifest': text, 'signature': signature.read_text(encoding='utf-8')}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--tag', required=True)
    parser.add_argument('--repository', required=True)
    parser.add_argument('--dist', type=Path, default=Path('dist'))
    parser.add_argument('--notes', type=Path, required=True)
    args = parser.parse_args()
    version = version_for_tag(args.tag)
    base = f'https://github.com/{args.repository}/releases/download/{args.tag}'
    notes = args.notes.read_text(encoding='utf-8')
    document = manifest(args.dist, version, base, notes)
    signed = sign_manifest(document, os.environ['MINISIGN_SECRET_KEY'], os.environ.get('MINISIGN_PASSWORD', ''))
    appcast(args.dist, version, base, notes)
    (args.dist / 'update.json').write_text(json.dumps(signed, ensure_ascii=False), encoding='utf-8')


if __name__ == '__main__':
    main()
