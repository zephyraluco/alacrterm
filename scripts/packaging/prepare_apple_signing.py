import base64, os
from pathlib import Path
root = Path(os.environ['RUNNER_TEMP'])
for name, output in [('APPLE_CERTIFICATE_P12', 'certificate.p12'), ('APPLE_API_KEY_P8', 'notary.p8')]:
    path = root / output
    path.write_bytes(base64.b64decode(os.environ[name], validate=True))
    path.chmod(0o600)
with open(os.environ['GITHUB_ENV'], 'a') as env:
    for name in ['APPLE_SIGNING_IDENTITY', 'APPLE_NOTARY_KEY_ID', 'APPLE_NOTARY_ISSUER']:
        value = os.environ[name]
        if '\n' in value or '\r' in value:
            raise ValueError('Invalid signing configuration')
        env.write(f'{name}={value}\n')
    env.write(f'APPLE_NOTARY_KEY={root / "notary.p8"}\n')
