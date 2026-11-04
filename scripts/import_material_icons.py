#!/usr/bin/env python3
"""Import SVGs and associations from an official Material Icon Theme VSIX.

Download a release with gh release download, then pass the VSIX path here.
"""
import argparse
import json
from pathlib import Path, PurePosixPath
import zipfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('vsix', type=Path)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
destination = root / 'assets/icons/material'
fields = ('file', 'folder', 'folderExpanded', 'fileNames', 'fileExtensions',
          'folderNames', 'folderNamesExpanded')
with zipfile.ZipFile(args.vsix) as archive:
    package = json.loads(archive.read('extension/package.json'))
    source = json.loads(archive.read('extension/dist/material-icons.json'))
    manifest = {key: source[key] for key in fields}
    manifest['light'] = {key: source['light'][key] for key in fields if key in source['light']}
    identifiers = set()
    for theme in (manifest, manifest['light']):
        for key in fields:
            value = theme.get(key, {})
            identifiers.update(value.values() if isinstance(value, dict) else [value])
    icons = {}
    for identifier in sorted(identifiers):
        filename = PurePosixPath(source['iconDefinitions'][identifier]['iconPath']).name
        icons[identifier] = archive.read('extension/icons/' + filename)
    license_path = next(name for name in archive.namelist()
                        if name.lower() in ('extension/license', 'extension/license.md', 'extension/license.txt'))
    license_text = archive.read(license_path)
    manifest['iconDefinitions'] = {
        name: 'icons/material/svg/' + name + '.svg' for name in sorted(icons)
    }
    # Read and validate the archive before replacing the existing imported files.
    svg_dir = destination / 'svg'
    svg_dir.mkdir(parents=True, exist_ok=True)
    for path in svg_dir.glob('*.svg'):
        path.unlink()
    for name, data in icons.items():
        (svg_dir / (name + '.svg')).write_bytes(data)
    (destination / 'manifest.json').write_text(json.dumps(manifest, sort_keys=True, separators=(',', ':')) + '\n')
    (root / 'docs/material-icons.md').write_text(
        '# Material Icon Theme assets\n\n'
        'Source: https://github.com/material-extensions/vscode-material-icon-theme\n\n'
        f'Official release: v{package["version"]} (MIT; full notice below).\n\n'
        'SVGs in `assets/icons/material/svg/` are copied unchanged from the official VSIX. '
        '`assets/icons/material/manifest.json` contains the default '
        'file/folder associations and light variants; language IDs and workspace root overrides '
        'are omitted because this application lists remote filesystem entries.\n\n'
        'Update from the repository root:\n\n```sh\n'
        f'gh release download v{package["version"]} --repo material-extensions/vscode-material-icon-theme '
        f'--pattern material-icon-theme-{package["version"]}.vsix --dir /tmp\n'
        f'python3 scripts/import_material_icons.py /tmp/material-icon-theme-{package["version"]}.vsix\n```\n\n'
        '## License\n\n' + license_text.decode().rstrip() + '\n'
    )
print(f'Imported {len(icons)} icons from Material Icon Theme {package["version"]}')
