"""Sign the final upgrade zip with Sparkle's official signing tool."""
import base64
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile


def main():
    packages = list(Path('dist').glob('*.app.zip'))
    if len(packages) != 1:
        raise ValueError('Expected exactly one macOS upgrade zip')
    secret = base64.b64decode(os.environ['SPARKLE_PRIVATE_KEY'].strip(), validate=True)
    public = base64.b64decode(os.environ['SPARKLE_PUBLIC_KEY'].strip(), validate=True)
    if len(secret) not in {64, 96} or len(public) != 32 or secret[-32:] != public:
        raise ValueError('Sparkle private key does not match the embedded public key')
    with tempfile.TemporaryDirectory() as temporary:
        key = Path(temporary) / 'sparkle.key'
        key.write_text(os.environ['SPARKLE_PRIVATE_KEY'], encoding='utf-8')
        key.chmod(0o600)
        tool = Path(os.environ['SPARKLE_FRAMEWORK_DIR']) / 'bin/sign_update'
        result = subprocess.run([str(tool), '--ed-key-file', str(key), str(packages[0])], text=True, check=True, capture_output=True)
    signature = re.search(r'sparkle:edSignature="([^"]+)"', result.stdout)
    length = re.search(r'length="(\d+)"', result.stdout)
    if not signature or not length or int(length[1]) != packages[0].stat().st_size:
        raise ValueError('Unexpected Sparkle sign_update output')
    if len(base64.b64decode(signature[1], validate=True)) != 64:
        raise ValueError('Invalid Sparkle signature length')
    Path('dist/sparkle-signature.json').write_text(json.dumps({'signature': signature[1], 'length': int(length[1])}), encoding='utf-8')


if __name__ == '__main__':
    main()
